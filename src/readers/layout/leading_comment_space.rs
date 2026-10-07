//! `Layout/LeadingCommentSpace`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags line comments whose text starts right after the comment marker: `--text`, `---text`
/// and `//text` should be `-- text`, `--- text` and `// text`.
///
/// Block comments (`--[[ ]]`, `/* */`) and comments made only of marker characters (`------`)
/// are exempt.
pub struct LeadingCommentSpace;

impl Reader for LeadingCommentSpace {
    fn name(&self) -> &'static str {
        "Layout/LeadingCommentSpace"
    }

    fn description(&self) -> &'static str {
        "Checks for a space after the comment marker."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for token in source.code_tokens() {
            if token.kind != (TokenKind::Comment { long: false }) {
                continue;
            }
            let text = source.text_of(token);
            let (marker, prefix) = if text.starts_with("--") {
                ('-', "--")
            } else if text.starts_with("//") {
                ('/', "//")
            } else {
                continue;
            };
            let body = text.trim_start_matches(marker);
            if body.is_empty() || body.starts_with([' ', '\t']) {
                continue;
            }
            let offset = token.start + text.len() - body.len();
            ctx.add_offense_with_fix(
                token.range(),
                format!("Missing space after `{prefix}`."),
                vec![Edit::insert(offset, " ")],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(LeadingCommentSpace)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/LeadingCommentSpace";

    #[test]
    fn flags_missing_space() {
        expect_offense(READER, "--text\n", 1, 1, "Missing space after `--`.");
        expect_offense(READER, "---text\n", 1, 1, "Missing space after `--`.");
        expect_offense(READER, "x = 1 --text\n", 1, 7, "Missing space after `--`.");
        expect_offense(READER, "//text\n", 1, 1, "Missing space after `//`.");
        expect_offense(READER, "--[ not long\n", 1, 1, "Missing space after `--`.");
    }

    #[test]
    fn accepts_spaced_and_exempt_comments() {
        expect_no_offenses(READER, "-- text\n--- Doc.\n-- @param x [Type]\n");
        expect_no_offenses(READER, "--\n---\n------------\n//\n");
        expect_no_offenses(READER, "--\ttext\n// text\n");
        expect_no_offenses(READER, "--[[long]]\n--[==[x]==]\n/*x*/\n");
        expect_no_offenses(READER, "s = '--text'\n");
        expect_no_offenses(READER, "x = 1 --\r\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "--text\n", "-- text\n");
        expect_correction(READER, "---Doc.\n", "--- Doc.\n");
        expect_correction(READER, "x() //note\n", "x() // note\n");
        expect_correction(READER, "-----x\n", "----- x\n");
        expect_correction(READER, "--text\r\n", "-- text\r\n");
    }
}
