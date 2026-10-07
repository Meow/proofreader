//! `Layout/SpaceInsideParens`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::inside_gaps;
use crate::token::TokenKind;

/// Flags whitespace directly inside `(` and `)` on the same line, including `( )`.
pub struct SpaceInsideParens;

impl Reader for SpaceInsideParens {
    fn name(&self) -> &'static str {
        "Layout/SpaceInsideParens"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces inside parentheses."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for (_, gap) in inside_gaps(source, TokenKind::LParen, TokenKind::RParen) {
            if gap.is_empty() {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Space inside parentheses detected.",
                vec![Edit::remove(gap.range())],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceInsideParens)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceInsideParens";
    const MESSAGE: &str = "Space inside parentheses detected.";

    #[test]
    fn flags_spaces_inside_parens() {
        expect_offense(READER, "f( a)\n", 1, 3, MESSAGE);
        expect_offense(READER, "f(a  )\n", 1, 4, MESSAGE);
        expect_offense(READER, "f( )\n", 1, 3, MESSAGE);
        expect_offenses(
            READER,
            "x = ( a + b )\n",
            &[(1, 6, MESSAGE), (1, 12, MESSAGE)],
        );
    }

    #[test]
    fn accepts_tight_parens_and_line_breaks() {
        expect_no_offenses(READER, "f(a, (b))\nf()\nf(\n  a\n)\nf(a,\n  b\n  )\n");
        expect_no_offenses(READER, "f( -- note\n  a)\ns = '( a )'\n-- ( a )\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "f( a, ( b ) )\nf( )\n", "f(a, (b))\nf()\n");
    }
}
