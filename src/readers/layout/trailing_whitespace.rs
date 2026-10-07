//! `Layout/TrailingWhitespace`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags spaces and tabs at the end of a line, except when they are part of a string.
pub struct TrailingWhitespace;

impl Reader for TrailingWhitespace {
    fn name(&self) -> &'static str {
        "Layout/TrailingWhitespace"
    }

    fn description(&self) -> &'static str {
        "Checks for trailing whitespace."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for (number, line) in source.lines() {
            let content = line.strip_suffix('\r').unwrap_or(line);
            let trimmed = content.trim_end_matches([' ', '\t']);
            if trimmed.len() == content.len() {
                continue;
            }
            let line_start = source.line_range(number).start;
            let range = line_start + trimmed.len()..line_start + content.len();
            if source
                .token_at(range.start)
                .is_some_and(|token| token.kind.is_string())
            {
                continue;
            }
            ctx.add_offense_with_fix(
                range.clone(),
                "Trailing whitespace detected.",
                vec![Edit::remove(range)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(TrailingWhitespace)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/TrailingWhitespace";

    #[test]
    fn flags_trailing_spaces_and_tabs() {
        expect_offense(
            READER,
            "local a = 1  \n",
            1,
            12,
            "Trailing whitespace detected.",
        );
        expect_offense(
            READER,
            "local a = 1\t\n",
            1,
            12,
            "Trailing whitespace detected.",
        );
        expect_offense(
            READER,
            "x()\n   \ny()\n",
            2,
            1,
            "Trailing whitespace detected.",
        );
        expect_offense(READER, "x() \r\n", 1, 4, "Trailing whitespace detected.");
        expect_offense(READER, "x() ", 1, 4, "Trailing whitespace detected.");
    }

    #[test]
    fn offense_covers_the_whitespace() {
        let offenses = inspect(READER, "a = 1 \t \n");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].range, 5..8);
        assert!(offenses[0].correctable());
    }

    #[test]
    fn accepts_clean_code() {
        expect_no_offenses(READER, "local a = 1\n\nlocal b = 2\n");
        expect_no_offenses(READER, "");
        expect_no_offenses(READER, "x()\r\n");
    }

    #[test]
    fn ignores_whitespace_inside_strings() {
        expect_no_offenses(READER, "local s = [[\nfoo   \nbar  \n]]\n");
        expect_no_offenses(READER, "local s = [[foo   \n]]\n");
        expect_no_offenses(READER, "local s = 'a\\\n  b'\n");
        expect_offense(
            READER,
            "local s = [[\nx]]  \n",
            2,
            4,
            "Trailing whitespace detected.",
        );
    }

    #[test]
    fn flags_whitespace_in_comments() {
        expect_offense(READER, "-- note  \n", 1, 8, "Trailing whitespace detected.");
        expect_offense(
            READER,
            "--[[\nlong  \n]]\n",
            2,
            5,
            "Trailing whitespace detected.",
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "local a = 1  \n", "local a = 1\n");
        expect_correction(READER, "a \t\n  \nb \r\n", "a\n\nb\r\n");
        expect_correction(READER, "s = [[x  \n]] \n", "s = [[x  \n]]\n");
    }
}
