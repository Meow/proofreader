//! `Layout/TrailingEmptyLines`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags a missing final newline and blank lines at the end of the file.
pub struct TrailingEmptyLines;

impl Reader for TrailingEmptyLines {
    fn name(&self) -> &'static str {
        "Layout/TrailingEmptyLines"
    }

    fn description(&self) -> &'static str {
        "Checks that the file ends with exactly one newline."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let text = source.text.as_str();
        if text.is_empty() {
            return;
        }
        let content_end = text.trim_end_matches([' ', '\t', '\r', '\n']).len();
        if content_end < text.len()
            && content_end > 0
            && source.token_at(content_end).is_some_and(|token| {
                (token.kind.is_string() || token.kind.is_comment()) && token.end == text.len()
            })
        {
            return;
        }
        let tail = &text[content_end..];
        let Some(first_newline) = tail.find('\n') else {
            ctx.add_offense_with_fix(
                text.len()..text.len(),
                "Final newline missing.",
                vec![Edit::insert(text.len(), "\n")],
            );
            return;
        };
        let blank_lines = tail.matches('\n').count() - 1;
        if blank_lines == 0 {
            return;
        }
        let range = content_end + first_newline + 1..text.len();
        let noun = if blank_lines == 1 { "line" } else { "lines" };
        ctx.add_offense_with_fix(
            range.clone(),
            format!("{blank_lines} trailing blank {noun} detected."),
            vec![Edit::remove(range)],
        );
    }
}

inventory::submit! { Registration(|| Box::new(TrailingEmptyLines)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/TrailingEmptyLines";

    #[test]
    fn accepts_a_single_final_newline() {
        expect_no_offenses(READER, "x()\n");
        expect_no_offenses(READER, "x()\n\ny()\n");
        expect_no_offenses(READER, "x()\r\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_a_missing_final_newline() {
        expect_offense(READER, "x()", 1, 4, "Final newline missing.");
        expect_offense(READER, "a\nend", 2, 4, "Final newline missing.");
        expect_correction(READER, "x()", "x()\n");
    }

    #[test]
    fn flags_trailing_blank_lines() {
        expect_offense(READER, "x()\n\n", 2, 1, "1 trailing blank line detected.");
        expect_offense(
            READER,
            "x()\n\n\n\n",
            2,
            1,
            "3 trailing blank lines detected.",
        );
        expect_offense(READER, "x()\n  \n", 2, 1, "1 trailing blank line detected.");
        expect_offense(
            READER,
            "x()\r\n\r\n",
            2,
            1,
            "1 trailing blank line detected.",
        );
    }

    #[test]
    fn autocorrects_trailing_blank_lines() {
        expect_correction(READER, "x()\n\n\n", "x()\n");
        expect_correction(READER, "x()\n \t\n\n", "x()\n");
        expect_correction(READER, "x()  \n\n", "x()  \n");
        expect_correction(READER, "x()\r\n\r\n", "x()\r\n");
        expect_correction(READER, "\n\n", "\n");
    }

    #[test]
    fn ignores_unterminated_long_tokens() {
        expect_no_offenses(READER, "s = [[abc\n\n\n");
        expect_no_offenses(READER, "--[[ note\n\n");
        expect_offense(
            READER,
            "s = [[abc\n\n]]\n\n",
            4,
            1,
            "1 trailing blank line detected.",
        );
    }
}
