//! `Layout/SpaceAfterNot`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags whitespace between `!` and its operand (`! x` should be `!x`).
pub struct SpaceAfterNot;

impl Reader for SpaceAfterNot {
    fn name(&self) -> &'static str {
        "Layout/SpaceAfterNot"
    }

    fn description(&self) -> &'static str {
        "Checks for space between `!` and its operand."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let tokens = &source.tokens;
        for (index, token) in tokens.iter().enumerate() {
            if token.kind != TokenKind::Bang {
                continue;
            }
            let gap_end = tokens[index + 1..]
                .iter()
                .take_while(|next| matches!(next.kind, TokenKind::Space | TokenKind::Tab))
                .last()
                .map_or(token.end, |space| space.end);
            if gap_end == token.end {
                continue;
            }
            let operand_follows = source.token_at(gap_end).is_some_and(|next| {
                !next.is_trivia() && !next.kind.is_comment() && next.kind != TokenKind::Eof
            });
            if !operand_follows {
                continue;
            }
            ctx.add_offense_with_fix(
                token.start..gap_end,
                "Do not leave space between `!` and its operand.",
                vec![Edit::remove(token.end..gap_end)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceAfterNot)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceAfterNot";
    const MESSAGE: &str = "Do not leave space between `!` and its operand.";

    #[test]
    fn flags_space_after_bang() {
        expect_offense(READER, "if ! x then end\n", 1, 4, MESSAGE);
        expect_offense(READER, "y = !\tx\n", 1, 5, MESSAGE);
        expect_offense(READER, "y = !  (a or b)\n", 1, 5, MESSAGE);
    }

    #[test]
    fn accepts_other_uses() {
        expect_no_offenses(READER, "if !x then end\n");
        expect_no_offenses(READER, "if a != b then end\n");
        expect_no_offenses(READER, "y = !!x\n");
        expect_no_offenses(READER, "s = '! x'\n");
        expect_no_offenses(READER, "y = !\n  x\n");
        expect_no_offenses(READER, "y = ! -- c\n  x\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "if ! x then end\n", "if !x then end\n");
        expect_correction(READER, "y = ! \t (a)\n", "y = !(a)\n");
        expect_correction(READER, "y = ! ! x\n", "y = !!x\n");
    }
}
