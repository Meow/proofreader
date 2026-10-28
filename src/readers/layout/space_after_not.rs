//! `Layout/SpaceAfterNot`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::is_binary_operator;
use crate::token::TokenKind;

/// Flags whitespace between `!` and its operand (`! x` should be `!x`), and likewise after the
/// bitwise not `~` (`~ x` should be `~x`); a `~` between two operands is the binary xor and left
/// to `Layout/SpaceAroundOperators`.
pub struct SpaceAfterNot;

impl Reader for SpaceAfterNot {
    fn name(&self) -> &'static str {
        "Layout/SpaceAfterNot"
    }

    fn description(&self) -> &'static str {
        "Checks for space between `!` (or a unary `~`) and its operand."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let tokens = &source.tokens;
        for (code_index, &index) in source.code_token_indexes().iter().enumerate() {
            let token = &tokens[index];
            let unary = match token.kind {
                TokenKind::Bang => true,
                TokenKind::Tilde => !is_binary_operator(source, code_index),
                _ => false,
            };
            if !unary {
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
                format!(
                    "Do not leave space between `{}` and its operand.",
                    source.text_of(token)
                ),
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

    #[test]
    fn checks_the_bitwise_not() {
        let message = "Do not leave space between `~` and its operand.";
        expect_offense(READER, "y = ~ x\n", 1, 5, message);
        expect_offense(READER, "y = a & ~ (b | c)\n", 1, 9, message);
        expect_no_offenses(
            READER,
            "y = ~x & ~~z\ny = a ~ b\nx ~= 1\nif a ~= b then end\n",
        );
        expect_correction(READER, "y = ~ ~ x\n", "y = ~~x\n");
    }

    #[test]
    fn checks_lua_before_markup() {
        expect_correction_lumen(READER, "return ! <view/>\n", "return !<view/>\n");
        expect_no_offenses_lumen(READER, "return <text>! x</text>\n");
    }
}
