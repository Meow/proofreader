//! `Layout/SpaceBeforeParen`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{gap_before, is_callee_end};
use crate::token::TokenKind;

/// Flags whitespace between a function name (or a `)`, `]` or string ending a call target) and
/// the `(` that opens its arguments or parameters on the same line.
///
/// Keywords before `(` (`if (`, `return (`, `and (`, `function (`) are not affected.
pub struct SpaceBeforeParen;

impl Reader for SpaceBeforeParen {
    fn name(&self) -> &'static str {
        "Layout/SpaceBeforeParen"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces between a function name and its opening parenthesis."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for code_index in 0..source.code_token_indexes().len() {
            if source
                .code_token(code_index)
                .is_none_or(|token| token.kind != TokenKind::LParen)
            {
                continue;
            }
            let Some(gap) = gap_before(source, code_index) else {
                continue;
            };
            if gap.is_empty() || !is_callee_end(gap.before.kind) {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Do not put a space between a function name and the opening parenthesis.",
                vec![Edit::remove(gap.range())],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceBeforeParen)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceBeforeParen";
    const MESSAGE: &str = "Do not put a space between a function name and the opening parenthesis.";

    #[test]
    fn flags_space_before_call_and_definition_parens() {
        expect_offense(READER, "foo (1)\n", 1, 4, MESSAGE);
        expect_offense(READER, "function foo (a) end\n", 1, 13, MESSAGE);
        expect_offense(READER, "local function foo\t(a) end\n", 1, 19, MESSAGE);
        expect_offense(READER, "a:b (c)\n", 1, 4, MESSAGE);
        expect_offense(READER, "f(x) (y)\n", 1, 5, MESSAGE);
        expect_offense(READER, "t[1] (y)\n", 1, 5, MESSAGE);
        expect_offense(READER, "f 'x' (y)\n", 1, 6, MESSAGE);
    }

    #[test]
    fn accepts_keywords_before_parens() {
        expect_no_offenses(
            READER,
            "if (a) then end\nreturn (a)\nx = a and (b) or (c)\nf = function (x) end\nx = -(y)\n",
        );
        expect_no_offenses(READER, "x = { (a) }\nf(a, (b))\nx = a + (b)\n");
    }

    #[test]
    fn accepts_calls_without_spaces() {
        expect_no_offenses(READER, "foo(1)\nfunction foo(a) end\nlocal s = 'foo (1)'\n");
        expect_no_offenses(READER, "local x = foo\n(bar)()\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "function foo  (a) bar (a) end\n",
            "function foo(a) bar(a) end\n",
        );
    }
}
