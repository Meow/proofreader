//! `Performance/TableInsertAppend`.

use std::ops::Range;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, FieldKey, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::performance::hot_paths::{abbreviated, is_repeatable};
use crate::source::Source;
use crate::token::Token;

/// Flags `table.insert(t, v)` standing as a statement with exactly two arguments, which
/// `t[#t + 1] = v` does without calling a function. Only tables that can be written twice are
/// flagged: a name, or fields and indexes over a name whose keys are literals or names (`t`,
/// `self.items`, `lists[id]`). Calls with a `...` value (which may stand for any number of
/// values), and calls where `table` is a local, are left alone. The fix writes
/// `t[#t + 1] = v`, unless a comment lies inside the call or the value calls a function and
/// mentions the table's root name (the call could grow the table before `#t` is taken). A call
/// as the value is assumed to return one value; unlike `table.insert`, the assignment goes
/// through a `__newindex` metamethod. Files that do not parse are skipped.
///
/// Disabled by default: `table.insert` is the default style, and this reader is for code bases
/// that prefer the append.
pub struct TableInsertAppend;

impl Reader for TableInsertAppend {
    fn name(&self) -> &'static str {
        "Performance/TableInsertAppend"
    }

    fn description(&self) -> &'static str {
        "Checks for `table.insert(t, v)` that `t[#t + 1] = v` does without a function call."
    }

    fn default_enabled(&self) -> bool {
        false
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
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, message, replacement) in finder.found {
            match replacement {
                Some(replacement) => {
                    ctx.add_offense_with_fix(
                        range.clone(),
                        message,
                        vec![Edit::replace(range, replacement)],
                    );
                }
                None => {
                    ctx.add_offense(range, message);
                }
            }
        }
    }
}

/// Collects the appending `table.insert` statements, with their replacement when it is safe.
struct Finder<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    found: Vec<(Range<usize>, String, Option<String>)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::Call(call) = &statement.kind else {
            return;
        };
        let ExpressionKind::Call { callee, arguments } = &call.kind else {
            return;
        };
        let ExpressionKind::Field { object, .. } = &callee.kind else {
            return;
        };
        let ExpressionKind::Name(library) = &object.kind else {
            return;
        };
        if callee.dotted_name(self.source).as_deref() != Some("table.insert")
            || self
                .resolution
                .reference_at(library.start)
                .is_none_or(|reference| reference.variable.is_some())
        {
            return;
        }
        let [table, value] = arguments.values.as_slice() else {
            return;
        };
        if !is_repeatable(table) || value.kind == ExpressionKind::Vararg {
            return;
        }
        let table_text = &self.source.text[table.range.clone()];
        let value_text = &self.source.text[value.range.clone()];
        let message = format!(
            "Use `{table}[#{table} + 1] = {value}` instead of `table.insert({table}, {value})`.",
            table = abbreviated(table_text),
            value = abbreviated(value_text),
        );
        let root = root_name(table).map(|root| self.source.text_of(root));
        let unsafe_value = any_part(value, &|part| part.is_call())
            && any_part(value, &|part| {
                matches!(&part.kind, ExpressionKind::Name(token)
                    if Some(self.source.text_of(token)) == root)
            });
        let replacement = (!unsafe_value && !self.source.has_comment_in(&call.range))
            .then(|| format!("{table_text}[#{table_text} + 1] = {value_text}"));
        self.found.push((call.range.clone(), message, replacement));
    }
}

/// The name token a name, field or index chain starts with (`self` for `self.items`).
fn root_name(expression: &Expression) -> Option<&Token> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some(token),
        ExpressionKind::Field { object, .. } | ExpressionKind::Index { object, .. } => {
            root_name(object)
        }
        _ => None,
    }
}

/// Whether `test` holds for `expression` or any expression inside it, outside the bodies of the
/// functions it defines.
fn any_part(expression: &Expression, test: &dyn Fn(&Expression) -> bool) -> bool {
    if test(expression) {
        return true;
    }
    match &expression.kind {
        ExpressionKind::Table(table) => table.fields.iter().any(|field| {
            matches!(&field.key, FieldKey::Bracketed(key) if any_part(key, test))
                || any_part(&field.value, test)
        }),
        ExpressionKind::Markup(splices) => splices.iter().any(|splice| any_part(splice, test)),
        ExpressionKind::Parenthesized(inner)
        | ExpressionKind::Field { object: inner, .. }
        | ExpressionKind::Unary { operand: inner, .. } => any_part(inner, test),
        ExpressionKind::Index { object, key } => any_part(object, test) || any_part(key, test),
        ExpressionKind::Binary { left, right, .. } => any_part(left, test) || any_part(right, test),
        ExpressionKind::Call {
            callee: object,
            arguments,
        }
        | ExpressionKind::MethodCall {
            object, arguments, ..
        } => {
            any_part(object, test)
                || arguments
                    .values
                    .iter()
                    .any(|argument| any_part(argument, test))
        }
        _ => false,
    }
}

inventory::submit! { Registration(|| Box::new(TableInsertAppend)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/TableInsertAppend";

    #[test]
    fn is_disabled_by_default() {
        let reader = crate::reader::reader_named(READER).expect("registered");
        assert!(!reader.default_enabled());
    }

    #[test]
    fn flags_appending_inserts() {
        expect_offense(
            READER,
            "local t = {}\ntable.insert(t, 5)\n",
            2,
            1,
            "Use `t[#t + 1] = 5` instead of `table.insert(t, 5)`.",
        );
        expect_offense(
            READER,
            "table.insert(self.items, { name = name, value = value, extra = true })\n",
            1,
            1,
            "Use `self.items[#self.items + 1] = ...` instead of `table.insert(self.items, ...)`.",
        );
    }

    #[test]
    fn corrects_to_an_indexed_assignment() {
        expect_correction(
            READER,
            "table.insert(t, v)\ntable.insert(self.items, {\n  a = 1,\n})\n\
             if x then table.insert(data[id].list, f(x)) end\ntable.insert(t, #t)\n",
            "t[#t + 1] = v\nself.items[#self.items + 1] = {\n  a = 1,\n}\n\
             if x then data[id].list[#data[id].list + 1] = f(x) end\nt[#t + 1] = #t\n",
        );
        expect_correction(
            READER,
            "local a = {} table.insert(a, 1); table.insert(a, 'x')\n",
            "local a = {} a[#a + 1] = 1; a[#a + 1] = 'x'\n",
        );
    }

    #[test]
    fn keeps_calls_that_could_grow_the_table() {
        let src = "table.insert(t, build(t))\ntable.insert(self.list, self:Make())\n";
        assert_eq!(inspect(READER, src).len(), 2);
        assert_eq!(autocorrect(READER, src), src);
        expect_correction(
            READER,
            "table.insert(status, { status = f(x), n = list.status })\n",
            "status[#status + 1] = { status = f(x), n = list.status }\n",
        );
    }

    #[test]
    fn keeps_calls_with_comments() {
        let src = "table.insert(t, -- the value\n  v)\n";
        assert_eq!(inspect(READER, src).len(), 1);
        assert_eq!(autocorrect(READER, src), src);
    }

    #[test]
    fn ignores_other_inserts() {
        expect_no_offenses(
            READER,
            "table.insert(t, 1, v)\ntable.insert(t, ...)\ntable.insert(get(), v)\n\
             table.insert(a[i + 1], v)\nlocal x = table.insert(t, v)\nf(table.insert(t, v))\n\
             table.insert(t)\ntbl.insert(t, v)\n",
        );
        expect_no_offenses(
            READER,
            "local table = require('mytable')\ntable.insert(t, v)\n",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "table.insert(t, v)\nif then\n");
    }
}
