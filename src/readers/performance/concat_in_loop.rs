//! `Performance/ConcatInLoop`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags strings built with `..` in a loop: an assignment `s = s .. x` (the target is the
/// leftmost operand of the concatenation) or `s = x .. s` (the rightmost) inside the body of a
/// `for`, `while` or `repeat` loop, where `s` is a name or a field chain over a name (`self.text`)
/// declared outside the loop. Every such assignment copies the whole string built so far, so the
/// loop takes quadratic time; collecting the parts in a table and calling `table.concat` copies
/// them once. Targets declared inside the loop (locals of its body and loop variables, or fields
/// of them) start over every iteration and are left alone, and so are loops in a function
/// defined outside them. Each target is reported once per loop, at its first assignment, in the
/// outermost loop of the function that it outlives. Files that do not parse are skipped.
pub struct ConcatInLoop;

impl Reader for ConcatInLoop {
    fn name(&self) -> &'static str {
        "Performance/ConcatInLoop"
    }

    fn description(&self) -> &'static str {
        "Checks for strings built with `..` in a loop, which copies them every iteration."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let Some(resolution) = resolve(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            resolution: &resolution,
            bodies: HashMap::new(),
            loops: vec![Vec::new()],
            reported: HashSet::new(),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, target) in finder.found {
            ctx.add_offense(
                range,
                format!(
                    "Building `{target}` with `..` in a loop copies it every time; collect the parts in a table and `table.concat` them."
                ),
            );
        }
    }
}

/// Collects the first assignment building each string in each loop.
struct Finder<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    /// The loop statement range of every loop body, keyed by the body's address.
    bodies: HashMap<*const Block, Range<usize>>,
    /// For every function being walked (the file first), the ranges of the loops around the
    /// code being walked, outermost first.
    loops: Vec<Vec<Range<usize>>>,
    /// The `(loop start, target)` pairs already reported.
    reported: HashSet<(usize, String)>,
    found: Vec<(Range<usize>, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn enter_function(&mut self, _function: &'t Function) {
        self.loops.push(Vec::new());
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.loops.pop();
    }

    fn enter_block(&mut self, block: &'t Block) {
        if let Some(range) = self.bodies.get(&std::ptr::from_ref(block))
            && let Some(loops) = self.loops.last_mut()
        {
            loops.push(range.clone());
        }
    }

    fn leave_block(&mut self, block: &'t Block) {
        if self.bodies.contains_key(&std::ptr::from_ref(block))
            && let Some(loops) = self.loops.last_mut()
        {
            loops.pop();
        }
    }

    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::While { body, .. }
            | StatementKind::Repeat { body, .. }
            | StatementKind::NumericFor { body, .. }
            | StatementKind::GenericFor { body, .. } => {
                self.bodies
                    .insert(std::ptr::from_ref(body), statement.range.clone());
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in targets.iter().zip(values) {
                    self.check(statement, target, value);
                }
            }
            _ => {}
        }
    }
}

impl Finder<'_> {
    /// Records the assignment `statement` of `value` to `target` when it builds a string
    /// declared outside a loop around it.
    fn check(&mut self, statement: &Statement, target: &Expression, value: &Expression) {
        let Some(name) = target.dotted_name(self.source) else {
            return;
        };
        if !builds(value, &name, self.source) {
            return;
        }
        let Some(root) = root_name(target) else {
            return;
        };
        let declared = self
            .resolution
            .reference_at(root.start)
            .and_then(|reference| reference.variable)
            .map(|variable| self.resolution.variables[variable].token.start);
        let Some(outlived) = self.loops.last().and_then(|loops| {
            loops
                .iter()
                .find(|range| declared.is_none_or(|declared| !range.contains(&declared)))
        }) else {
            return;
        };
        if self.reported.insert((outlived.start, name.clone())) {
            self.found.push((statement.range.clone(), name));
        }
    }
}

/// Whether `value` is a concatenation whose leftmost or rightmost operand is the name or field
/// chain `name`.
fn builds(value: &Expression, name: &str, source: &Source) -> bool {
    if !is_concat(value) {
        return false;
    }
    let end = |mut operand: &Expression, leftmost: bool| {
        while let ExpressionKind::Binary {
            operator,
            left,
            right,
        } = &operand.kind
            && operator.kind == TokenKind::Concat
        {
            operand = if leftmost { left } else { right };
        }
        operand.dotted_name(source).as_deref() == Some(name)
    };
    end(value, true) || end(value, false)
}

/// Whether `expression` is a `..` operation.
fn is_concat(expression: &Expression) -> bool {
    matches!(&expression.kind, ExpressionKind::Binary { operator, .. } if operator.kind == TokenKind::Concat)
}

/// The name token a name or field chain starts with (`self` for `self.text`).
fn root_name(expression: &Expression) -> Option<&Token> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some(token),
        ExpressionKind::Field { object, .. } => root_name(object),
        _ => None,
    }
}

inventory::submit! { Registration(|| Box::new(ConcatInLoop)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/ConcatInLoop";

    /// The message for the target `target`.
    fn message(target: &str) -> String {
        format!(
            "Building `{target}` with `..` in a loop copies it every time; collect the parts in a table and `table.concat` them."
        )
    }

    #[test]
    fn flags_appending_and_prepending() {
        expect_offense(
            READER,
            "local s = ''\nfor i = 1, 10 do\n  s = s .. i .. ','\nend\n",
            3,
            3,
            &message("s"),
        );
        expect_offense(
            READER,
            "local s = ''\nwhile x do\n  s = x .. s\nend\n",
            3,
            3,
            &message("s"),
        );
        expect_offense(
            READER,
            "function PANEL:Build(lines)\n  for _, line in ipairs(lines) do\n    self.text = self.text .. line\n  end\nend\n",
            3,
            5,
            &message("self.text"),
        );
        expect_offense(
            READER,
            "repeat\n  out = out .. read()\nuntil done\n",
            2,
            3,
            &message("out"),
        );
    }

    #[test]
    fn reports_each_target_once_per_outermost_loop() {
        expect_offenses(
            READER,
            "local s, t = '', ''\nfor i = 1, 3 do\n  s = s .. i\n  for j = 1, 3 do\n    s = s .. j\n    t = t .. j\n  end\nend\n",
            &[(3, 3, &message("s")), (6, 5, &message("t"))],
        );
        expect_offenses(
            READER,
            "for i = 1, 3 do\n  local row = ''\n  for j = 1, 3 do\n    row = row .. j\n  end\n  print(row)\nend\n",
            &[(4, 5, &message("row"))],
        );
    }

    #[test]
    fn ignores_strings_built_within_one_iteration() {
        expect_no_offenses(
            READER,
            "for i = 1, 3 do\n  local s = 'a'\n  s = s .. i\n  local entry = {}\n  entry.name = entry.name .. i\nend\n\
             for _, name in ipairs(names) do\n  name = name .. '!'\nend\n",
        );
    }

    #[test]
    fn ignores_other_assignments() {
        expect_no_offenses(
            READER,
            "local s = ''\nfor i = 1, 3 do\n  s = a .. s .. b\n  s = (s .. a) .. b\n  s = t .. u\n  local u = s .. i\n  s = s + 1\n  t[i] = t[i] .. i\nend\n\
             s = s .. 'x'\nif x then s = s .. 'y' end\n",
        );
    }

    #[test]
    fn ignores_functions_defined_in_loops() {
        expect_no_offenses(
            READER,
            "local s = ''\nfor i = 1, 3 do\n  local function add(x) s = s .. x end\n  callbacks[i] = function() s = s .. i end\nend\n",
        );
        expect_offense(
            READER,
            "for i = 1, 3 do\n  f(function(list)\n    local s = ''\n    for _, x in ipairs(list) do s = s .. x end\n  end)\nend\n",
            4,
            33,
            &message("s"),
        );
    }
}
