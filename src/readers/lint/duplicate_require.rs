//! `Lint/DuplicateRequire`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Block, Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;

/// Flags a call that loads a module already loaded by an earlier call in the same or an
/// enclosing block.
///
/// The calls are those of the functions listed in `Functions` (default `require`) with a string
/// literal as their first argument. Calls in sibling blocks, such as the two branches of
/// `if SERVER then ... else ... end`, do not conflict, and a call whose result is indexed right
/// away (`require('x').y`) is not checked. Files that do not parse are skipped.
pub struct DuplicateRequire;

impl Reader for DuplicateRequire {
    fn name(&self) -> &'static str {
        "Lint/DuplicateRequire"
    }

    fn description(&self) -> &'static str {
        "Checks for modules that are required more than once."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![(
            "Functions",
            Yaml::Array(vec![Yaml::String("require".to_owned())]),
        )]
    }

    fn investigate(&self, ctx: &mut Context) {
        let functions = ctx.option_str_list("Functions", &["require"]);
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = VisibleCalls::new(source, |name: &str| {
            functions.iter().any(|function| function == name)
        });
        walk(&block, &mut finder);
        for duplicate in finder.duplicates {
            ctx.add_offense(
                duplicate.range,
                format!(
                    "`{}` is already required on line {}.",
                    duplicate.key, duplicate.first_line
                ),
            );
        }
    }
}

/// A call repeating an earlier call that is visible from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    /// The function called, as written (`util.AddNetworkString`).
    pub function: String,
    /// The literal first argument of the repeated call.
    pub key: String,
    /// The range of the repeated call.
    pub range: Range<usize>,
    /// The line of the earlier call.
    pub first_line: u32,
}

/// Finds calls of the functions `matches` accepts whose literal first argument (compared after
/// `normalize`) repeats an earlier such call in the same or an enclosing block, skipping calls
/// whose result is indexed or has a method called on it. Shared with
/// `Lint/DuplicateRegistration`.
pub struct VisibleCalls<'s, M> {
    source: &'s Source,
    matches: M,
    normalize: fn(&str, &str) -> String,
    next_block: usize,
    active: Vec<usize>,
    seen: Vec<(String, usize, u32)>,
    indexed: HashSet<Range<usize>>,
    /// The repeated calls in source order.
    pub duplicates: Vec<Duplicate>,
}

impl<'s, M: Fn(&str) -> bool> VisibleCalls<'s, M> {
    /// A finder comparing the literal arguments as written.
    pub fn new(source: &'s Source, matches: M) -> Self {
        Self::with_key(source, matches, |_, argument| argument.to_owned())
    }

    /// A finder comparing `normalize(function, argument)`.
    pub fn with_key(source: &'s Source, matches: M, normalize: fn(&str, &str) -> String) -> Self {
        VisibleCalls {
            source,
            matches,
            normalize,
            next_block: 0,
            active: Vec::new(),
            seen: Vec::new(),
            indexed: HashSet::new(),
            duplicates: Vec::new(),
        }
    }
}

impl<'t, M: Fn(&str) -> bool> Visitor<'t> for VisibleCalls<'_, M> {
    fn enter_block(&mut self, _block: &'t Block) {
        self.active.push(self.next_block);
        self.next_block += 1;
    }

    fn leave_block(&mut self, _block: &'t Block) {
        if let Some(id) = self.active.pop() {
            self.seen.retain(|(_, block, _)| *block != id);
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        if let ExpressionKind::Field { object, .. }
        | ExpressionKind::Index { object, .. }
        | ExpressionKind::MethodCall { object, .. } = &expression.kind
        {
            self.indexed.insert(object.range.clone());
            return;
        }
        if self.indexed.contains(&expression.range) {
            return;
        }
        let Some((function, arguments)) = expression.as_call(self.source) else {
            return;
        };
        if !(self.matches)(&function) {
            return;
        }
        let Some(argument) = arguments
            .values
            .first()
            .and_then(|value| value.string_contents(self.source))
        else {
            return;
        };
        let key = (self.normalize)(&function, argument);
        if let Some((_, _, first_line)) = self.seen.iter().find(|(seen, _, _)| *seen == key) {
            self.duplicates.push(Duplicate {
                function,
                key: argument.to_owned(),
                range: expression.range.clone(),
                first_line: *first_line,
            });
            return;
        }
        let line = self.source.position(expression.range.start).0;
        let block = self.active.last().copied().unwrap_or(0);
        self.seen.push((key, block, line));
    }
}

inventory::submit! { Registration(|| Box::new(DuplicateRequire)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/DuplicateRequire";

    #[test]
    fn flags_repeated_requires() {
        expect_offense(
            READER,
            "require('a')\nrequire 'b'\nrequire [[a]]\n",
            3,
            1,
            "`a` is already required on line 1.",
        );
        expect_offense(
            READER,
            "local a = require('a')\n\nfunction f()\n  local b = require('a')\nend\n",
            4,
            13,
            "`a` is already required on line 1.",
        );
    }

    #[test]
    fn accepts_sibling_blocks_and_other_modules() {
        expect_no_offenses(
            READER,
            "if SERVER then\n  require('a')\nelse\n  require('a')\nend\n\
             function f() require('b') end\nfunction g() require('b') end\nrequire('c')\n",
        );
        expect_no_offenses(READER, "require(name)\nrequire(name)\n");
    }

    #[test]
    fn skips_indexed_results() {
        expect_no_offenses(
            READER,
            "local a = require('a').x\nlocal b = require('a')['y']\nrequire('a'):init()\n\
             local c = require('a')\n",
        );
    }

    #[test]
    fn honours_functions() {
        let src = "include('a.lua')\ninclude('a.lua')\nrequire('b')\nrequire('b')\n";
        expect_offense(READER, src, 4, 1, "`b` is already required on line 3.");
        let offenses = inspect_with(READER, src, "Functions: [include]");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].line, 2);
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(READER, "require('a')\nrequire('a')\nif then\n");
    }

    #[test]
    fn checks_lumen_templates() {
        expect_offense_lumen(
            READER,
            "local a = require('a')\nreturn <view>{require('a')}</view>\n",
            2,
            15,
            "`a` is already required on line 1.",
        );
    }
}
