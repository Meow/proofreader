//! `Layout/IndentationStyle`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags tabs in the leading indentation of a line; the fix replaces each tab with
/// `IndentationWidth` spaces.
///
/// Lines inside multi-line strings and whitespace-only lines (left to
/// `Layout/TrailingWhitespace`) are not checked.
pub struct IndentationStyle;

impl Reader for IndentationStyle {
    fn name(&self) -> &'static str {
        "Layout/IndentationStyle"
    }

    fn description(&self) -> &'static str {
        "Checks that indentation uses spaces, not tabs."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("IndentationWidth", Yaml::Integer(2))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let spaces = " ".repeat(ctx.option_usize("IndentationWidth", 2));
        let source = ctx.source;
        for (number, _) in source.lines() {
            let indentation = source.indentation(number);
            let (Some(first), Some(last)) = (indentation.find('\t'), indentation.rfind('\t'))
            else {
                continue;
            };
            if source.is_blank(number) {
                continue;
            }
            let line_start = source.line_range(number).start;
            if source
                .token_at(line_start)
                .is_some_and(|token| token.kind.is_string())
            {
                continue;
            }
            let range = line_start + first..line_start + last + 1;
            let replacement = indentation[first..=last].replace('\t', &spaces);
            ctx.add_offense_with_fix(
                range.clone(),
                "Tab detected in indentation.",
                vec![Edit::replace(range, replacement)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(IndentationStyle)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/IndentationStyle";

    #[test]
    fn flags_tabs_in_indentation() {
        expect_offense(
            READER,
            "if x then\n\ty()\nend\n",
            2,
            1,
            "Tab detected in indentation.",
        );
        expect_offense(
            READER,
            "do\n  \tx()\nend\n",
            2,
            3,
            "Tab detected in indentation.",
        );
        expect_offense(READER, "\t-- note\n", 1, 1, "Tab detected in indentation.");
    }

    #[test]
    fn ignores_other_tabs() {
        expect_no_offenses(READER, "x = 1\t-- note\n");
        expect_no_offenses(READER, "s = [[\n\tdata\n]]\n");
        expect_no_offenses(READER, "s = 'a\\\n\tb'\n");
        expect_no_offenses(READER, "x()\n\t\ny()\n");
        expect_no_offenses(READER, "  x()\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "do\n\tx()\nend\n", "do\n  x()\nend\n");
        expect_correction(READER, "do\n\t\tx()\nend\n", "do\n    x()\nend\n");
        expect_correction(READER, "do\n\t x()\nend\n", "do\n   x()\nend\n");
    }

    #[test]
    fn honours_indentation_width() {
        assert_eq!(
            autocorrect_with(READER, "do\n\tx()\nend\n", "IndentationWidth: 4"),
            "do\n    x()\nend\n"
        );
    }
}
