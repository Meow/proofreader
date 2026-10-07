//! `Layout/SpaceBeforeComment`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags a comment that directly touches the code before it on the same line (`x()-- note`).
pub struct SpaceBeforeComment;

impl Reader for SpaceBeforeComment {
    fn name(&self) -> &'static str {
        "Layout/SpaceBeforeComment"
    }

    fn description(&self) -> &'static str {
        "Checks for a space between code and a trailing comment."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for (index, token) in source.tokens.iter().enumerate() {
            if !token.kind.is_comment() {
                continue;
            }
            let Some(previous) = index.checked_sub(1).map(|before| &source.tokens[before]) else {
                continue;
            };
            if previous.is_trivia() || (previous.kind == TokenKind::Unknown && previous.start == 0)
            {
                continue;
            }
            ctx.add_offense_with_fix(
                token.range(),
                "Put a space before an end-of-line comment.",
                vec![Edit::insert(token.start, " ")],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceBeforeComment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceBeforeComment";
    const MESSAGE: &str = "Put a space before an end-of-line comment.";

    #[test]
    fn flags_comments_touching_code() {
        expect_offense(READER, "x()-- note\n", 1, 4, MESSAGE);
        expect_offense(READER, "x = 1--note\n", 1, 6, MESSAGE);
        expect_offense(READER, "x = 'a'--[[ note ]]\n", 1, 8, MESSAGE);
        expect_offense(READER, "x()// note\n", 1, 4, MESSAGE);
    }

    #[test]
    fn accepts_spaced_comments() {
        expect_no_offenses(READER, "x() -- note\n");
        expect_no_offenses(READER, "x()\t-- note\n");
        expect_no_offenses(READER, "-- note\n  -- indented\n");
        expect_no_offenses(READER, "\u{feff}-- note\n");
        expect_no_offenses(READER, "s = 'a--b'\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "x()-- note\n", "x() -- note\n");
        expect_correction(READER, "x = 1--note\ny()--z\n", "x = 1 --note\ny() --z\n");
    }
}
