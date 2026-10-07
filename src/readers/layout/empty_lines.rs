//! `Layout/EmptyLines`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags every blank line that directly follows another blank line.
///
/// Lines inside multi-line strings and comments are not blank lines, and blank lines at the
/// end of the file are left to `Layout/TrailingEmptyLines`.
pub struct EmptyLines;

impl Reader for EmptyLines {
    fn name(&self) -> &'static str {
        "Layout/EmptyLines"
    }

    fn description(&self) -> &'static str {
        "Checks for two or more consecutive blank lines."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let inside = source.lines_inside_multiline_tokens();
        let blank = |number: u32| source.is_blank(number) && !inside.contains(&number);
        let last_content = (1..=source.line_count())
            .rev()
            .find(|&number| !blank(number))
            .unwrap_or(0);
        for number in 2..last_content {
            if !blank(number) || !blank(number - 1) {
                continue;
            }
            let line = source.line_range(number);
            let removal = line.start..line.end + 1;
            ctx.add_offense_with_fix(
                line,
                "Extra blank line detected.",
                vec![Edit::remove(removal)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(EmptyLines)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLines";

    #[test]
    fn accepts_single_blank_lines() {
        expect_no_offenses(READER, "a()\n\nb()\n\nc()\n");
        expect_no_offenses(READER, "");
        expect_no_offenses(READER, "a()\n\n\n");
    }

    #[test]
    fn flags_consecutive_blank_lines() {
        expect_offense(READER, "a()\n\n\nb()\n", 3, 1, "Extra blank line detected.");
        expect_offenses(
            READER,
            "a()\n\n  \n\t\nb()\n",
            &[
                (3, 1, "Extra blank line detected."),
                (4, 1, "Extra blank line detected."),
            ],
        );
        expect_offense(READER, "\n\na()\n", 2, 1, "Extra blank line detected.");
        expect_offense(
            READER,
            "a()\r\n\r\n\r\nb()\r\n",
            3,
            1,
            "Extra blank line detected.",
        );
    }

    #[test]
    fn ignores_multiline_strings_and_comments() {
        expect_no_offenses(READER, "s = [[\n\n\n]]\n");
        expect_no_offenses(READER, "--[[\n\n\n]]\n");
        expect_offense(
            READER,
            "s = [[\n]]\n\n\nx()\n",
            4,
            1,
            "Extra blank line detected.",
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "a()\n\n\n\nb()\n", "a()\n\nb()\n");
        expect_correction(READER, "a()\n\n \n\nb()\n", "a()\n\nb()\n");
        expect_correction(READER, "a()\r\n\r\n\r\nb()\r\n", "a()\r\n\r\nb()\r\n");
    }
}
