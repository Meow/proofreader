//! `Style/CallParentheses`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    ArgumentStyle, Arguments, Expression, ExpressionKind, Visitor, parse, walk,
};

/// Checks whether calls with a single string or table literal argument use parentheses.
///
/// With `EnforcedStyle: always` (the default) `f 'x'`, `f { 1 }` and `obj:m 'x'` become
/// `f('x')`, `f({ 1 })` and `obj:m('x')`. With `omit` a call whose only argument is a string or
/// table literal drops its parentheses; `omit_for_strings` and `omit_for_tables` do so only for
/// that kind of argument and require parentheses for the other. The omitted form is written with
/// one space between the callee and the argument: `require 'x'`, `f { 1 }`. A parenthesised
/// literal (`f(('x'))`) is not a literal argument, and parentheses separated from their argument
/// by a line break (an argument list unfolded by `Layout/LineLength`) are kept. Calls with a
/// comment between the callee, the parentheses and the argument are reported but not corrected.
/// Files that do not parse are skipped. Disabled by default, as many code bases mix both forms
/// (Flux writes `require_relative 'x'` and `t'key'` but parenthesises other calls).
pub struct CallParentheses;

impl Reader for CallParentheses {
    fn name(&self) -> &'static str {
        "Style/CallParentheses"
    }

    fn description(&self) -> &'static str {
        "Checks the parentheses of calls with a single string or table argument."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("always".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let style = ctx.option_str("EnforcedStyle", "always");
        let (omit_strings, omit_tables) = match style {
            "omit" => (true, true),
            "omit_for_strings" => (true, false),
            "omit_for_tables" => (false, true),
            _ => (false, false),
        };
        let Ok(block) = parse(source) else {
            return;
        };
        let mut calls = Calls(Vec::new());
        walk(&block, &mut calls);
        for (callee_end, arguments) in calls.0 {
            match arguments.style {
                ArgumentStyle::Parenthesized => {
                    let [value] = arguments.values.as_slice() else {
                        continue;
                    };
                    let omit = match value.kind {
                        ExpressionKind::String(_) => omit_strings,
                        ExpressionKind::Table(_) => omit_tables,
                        _ => false,
                    };
                    if omit {
                        report_parentheses(ctx, callee_end, arguments, value);
                    }
                }
                ArgumentStyle::String if !omit_strings => {
                    report_missing(ctx, callee_end, arguments, "string");
                }
                ArgumentStyle::Table if !omit_tables => {
                    report_missing(ctx, callee_end, arguments, "table");
                }
                _ => {}
            }
        }
    }
}

/// Collects every call as the end of its callee (or method name) and its arguments.
struct Calls<'t>(Vec<(usize, &'t Arguments)>);

impl<'t> Visitor<'t> for Calls<'t> {
    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Call { callee, arguments } => {
                self.0.push((callee.range.end, arguments));
            }
            ExpressionKind::MethodCall {
                name, arguments, ..
            } => self.0.push((name.end, arguments)),
            _ => {}
        }
    }
}

/// Reports a parenless call whose `kind` argument needs parentheses.
fn report_missing(ctx: &mut Context, callee_end: usize, arguments: &Arguments, kind: &str) {
    let message = format!("Use parentheses around the {kind} argument of a call.");
    let gap = callee_end..arguments.range.start;
    let commented = ctx.source.has_comment_in(&gap);
    let offense = ctx.add_offense(arguments.range.clone(), message);
    if !commented {
        offense.with_fix(vec![
            Edit::replace(gap, "("),
            Edit::insert(arguments.range.end, ")"),
        ]);
    }
}

/// Reports the parentheses around the single literal `value` of a call.
fn report_parentheses(
    ctx: &mut Context,
    callee_end: usize,
    arguments: &Arguments,
    value: &Expression,
) {
    let kind = match value.kind {
        ExpressionKind::String(_) => "string",
        _ => "table",
    };
    let message = format!("Omit the parentheses around a single {kind} argument.");
    let before = callee_end..value.range.start;
    let after = value.range.end..arguments.range.end;
    let text = &ctx.source.text;
    if text[before.clone()].contains('\n') || text[after.clone()].contains('\n') {
        return;
    }
    let commented = ctx.source.has_comment_in(&before) || ctx.source.has_comment_in(&after);
    let offense = ctx.add_offense(arguments.range.clone(), message);
    if !commented {
        offense.with_fix(vec![Edit::replace(before, " "), Edit::remove(after)]);
    }
}

inventory::submit! { Registration(|| Box::new(CallParentheses)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/CallParentheses";
    const STRING: &str = "Use parentheses around the string argument of a call.";
    const TABLE: &str = "Use parentheses around the table argument of a call.";
    const OMIT_STRING: &str = "Omit the parentheses around a single string argument.";
    const OMIT_TABLE: &str = "Omit the parentheses around a single table argument.";

    #[test]
    fn flags_parenless_calls() {
        expect_offense(READER, "require 'x'\n", 1, 9, STRING);
        expect_offense(READER, "f{ 1 }\n", 1, 2, TABLE);
        expect_offense(READER, "obj:m [[x]]\n", 1, 7, STRING);
        expect_offenses(
            READER,
            "local a = f 'x' .. g { b = h 'y' }\n",
            &[(1, 13, STRING), (1, 22, TABLE), (1, 30, STRING)],
        );
    }

    #[test]
    fn looks_inside_the_extended_syntax() {
        expect_correction(
            READER,
            "f(|x| -> g 'a', c ? h { 1 } : nil, a ?? i 'b')\n",
            "f(|x| -> g('a'), c ? h({ 1 }) : nil, a ?? i('b'))\n",
        );
    }

    #[test]
    fn accepts_parenthesized_calls() {
        expect_no_offenses(
            READER,
            "require('x')\nf({ 1 })\nobj:m('x')\nf()\nf('a', 'b')\nlocal s = 'x'\n",
        );
    }

    #[test]
    fn adds_parentheses() {
        expect_correction(
            READER,
            "require 'x'\nf{ 1 }\nobj:m [[x]]\nf'a':g 'b'\nlocal t = f\n{ 1 }\n",
            "require('x')\nf({ 1 })\nobj:m([[x]])\nf('a'):g('b')\nlocal t = f({ 1 })\n",
        );
        expect_correction(READER, "f 'a' 'b'\n", "f('a')('b')\n");
        expect_correction(
            READER,
            "f { g 'x', h { 1 } }\n",
            "f({ g('x'), h({ 1 }) })\n",
        );
    }

    #[test]
    fn omits_parentheses() {
        let omit = "EnforcedStyle: omit";
        let offenses = inspect_with(READER, "require('x')\nf({ 1 })\nf(g)\nf(('x'))\n", omit);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(found, vec![(1, 8, OMIT_STRING), (2, 2, OMIT_TABLE)]);
        assert_eq!(
            autocorrect_with(
                READER,
                "require('x')\nf({ 1 })\nobj:m( 'x' )\nf('a'):g()\nf('a', 'b')\n",
                omit
            ),
            "require 'x'\nf { 1 }\nobj:m 'x'\nf 'a':g()\nf('a', 'b')\n"
        );
        assert!(inspect_with(READER, "require 'x'\nf { 1 }\n", omit).is_empty());
        assert!(inspect_with(READER, "f(\n  'x'\n)\ng(\n  { 1 }\n)\n", omit).is_empty());
        assert_eq!(
            autocorrect_with(READER, "g({\n  1\n})\n", omit),
            "g {\n  1\n}\n"
        );
    }

    #[test]
    fn omits_for_one_kind_only() {
        let strings = "EnforcedStyle: omit_for_strings";
        assert_eq!(
            autocorrect_with(READER, "require('x')\nf{ 1 }\nf({ 2 })\n", strings),
            "require 'x'\nf({ 1 })\nf({ 2 })\n"
        );
        let tables = "EnforcedStyle: omit_for_tables";
        assert_eq!(
            autocorrect_with(READER, "require 'x'\nf({ 1 })\ng('y')\n", tables),
            "require('x')\nf { 1 }\ng('y')\n"
        );
    }

    #[test]
    fn leaves_comments_alone() {
        let src = "f -- why\n  'x'\n";
        expect_offense(READER, src, 2, 3, STRING);
        assert_eq!(autocorrect(READER, src), src);
        let omit = "EnforcedStyle: omit";
        let src = "f( --[[ why ]] 'x')\n";
        assert_eq!(inspect_with(READER, src, omit).len(), 1);
        assert_eq!(autocorrect_with(READER, src, omit), src);
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(READER, "require 'x'\nif then\n");
    }

    #[test]
    fn checks_lua_in_lumen_templates() {
        expect_correction_lumen(
            READER,
            "return <view a={f 'x'}>{g{ 1 }}</view>\n",
            "return <view a={f('x')}>{g({ 1 })}</view>\n",
        );
    }
}
