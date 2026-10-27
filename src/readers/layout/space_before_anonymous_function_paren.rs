//! `Layout/SpaceBeforeAnonymousFunctionParen`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::gap_after;
use crate::token::TokenKind;

/// Checks the whitespace between the `function` keyword of an anonymous function and the `(` of
/// its parameters on the same line.
///
/// With `EnforcedStyle: no_space` (the default) it is written `function(a)`; with `space`,
/// `function (a)` with exactly one space. Named functions are left to `Layout/SpaceBeforeParen`,
/// and a `(` on the next line or after a comment is not checked.
pub struct SpaceBeforeAnonymousFunctionParen;

impl Reader for SpaceBeforeAnonymousFunctionParen {
    fn name(&self) -> &'static str {
        "Layout/SpaceBeforeAnonymousFunctionParen"
    }

    fn description(&self) -> &'static str {
        "Checks the spacing between `function` and the parameters of an anonymous function."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("no_space".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let spaced = ctx.option_str("EnforcedStyle", "no_space") == "space";
        for code_index in 0..source.code_token_indexes().len() {
            if source
                .code_token(code_index)
                .is_none_or(|token| token.kind != TokenKind::Function)
            {
                continue;
            }
            let Some(gap) =
                gap_after(source, code_index).filter(|gap| gap.after.kind == TokenKind::LParen)
            else {
                continue;
            };
            let (range, message, edit) = if !spaced {
                if gap.is_empty() {
                    continue;
                }
                (
                    gap.range(),
                    "Do not put a space between `function` and the opening parenthesis.",
                    Edit::remove(gap.range()),
                )
            } else if gap.is_empty() {
                (
                    gap.before.start..gap.after.end,
                    "Put a space between `function` and the opening parenthesis.",
                    Edit::insert(gap.range().start, " "),
                )
            } else if !gap.is_single_space(source) {
                (
                    gap.range(),
                    "Put exactly one space between `function` and the opening parenthesis.",
                    Edit::replace(gap.range(), " "),
                )
            } else {
                continue;
            };
            ctx.add_offense_with_fix(range, message, vec![edit]);
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceBeforeAnonymousFunctionParen)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceBeforeAnonymousFunctionParen";
    const NO_SPACE: &str = "Do not put a space between `function` and the opening parenthesis.";
    const MISSING: &str = "Put a space between `function` and the opening parenthesis.";
    const EXTRA: &str = "Put exactly one space between `function` and the opening parenthesis.";

    #[test]
    fn flags_spaced_anonymous_functions() {
        expect_offense(READER, "f = function (a) end\n", 1, 13, NO_SPACE);
        expect_offense(READER, "f(function\t() end)\n", 1, 11, NO_SPACE);
        expect_offense(READER, "t = { x = function  () end }\n", 1, 19, NO_SPACE);
    }

    #[test]
    fn accepts_unspaced_and_named_functions() {
        expect_no_offenses(
            READER,
            "f = function(a) end\nhook.Add('X', 'y', function() end)\n",
        );
        expect_no_offenses(READER, "function foo (a) end\nlocal function bar(a) end\n");
        expect_no_offenses(
            READER,
            "f = function\n(a) end\nf = function --[[x]] (a) end\n",
        );
        expect_no_offenses(READER, "s = 'function (a)'\n-- function (a)\n");
    }

    #[test]
    fn space_style() {
        let options = "EnforcedStyle: space";
        assert!(
            inspect_with(
                READER,
                "f = function (a) end\nfunction foo(a) end\n",
                options
            )
            .is_empty()
        );
        let offenses = inspect_with(
            READER,
            "f = function(a) end\ng = function  () end\n",
            options,
        );
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|o| (o.line, o.col, o.message.as_str()))
            .collect();
        assert_eq!(found, vec![(1, 5, MISSING), (2, 13, EXTRA)]);
        assert_eq!(
            autocorrect_with(READER, "f = function(a) g(function\t() end) end\n", options),
            "f = function (a) g(function () end) end\n"
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "f = function (a) g(function  () end) end\n",
            "f = function(a) g(function() end) end\n",
        );
    }

    #[test]
    fn checks_lua_inside_lumen_splices() {
        expect_correction_lumen(
            READER,
            "return <button on_click={function () end}>function (x)</button>\n",
            "return <button on_click={function() end}>function (x)</button>\n",
        );
    }
}
