//! `Style/StringLiterals`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::style::quotes::{to_double_quoted, to_single_quoted};
use crate::token::TokenKind;

/// Enforces one quote style for short string literals.
///
/// With `EnforcedStyle: single_quotes` (the default) a double-quoted string is allowed only when
/// its content contains a single quote; `double_quotes` is the mirror image. Long strings are
/// exempt. The fix swaps the quotes and unescapes the quote that no longer needs escaping.
pub struct StringLiterals;

impl Reader for StringLiterals {
    fn name(&self) -> &'static str {
        "Style/StringLiterals"
    }

    fn description(&self) -> &'static str {
        "Checks that string literals use the configured quotes."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("single_quotes".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let double = ctx.option_str("EnforcedStyle", "single_quotes") == "double_quotes";
        let (convert, message): (fn(&str) -> Option<String>, &str) = if double {
            (
                to_double_quoted,
                "Prefer double-quoted strings unless the string contains a double quote.",
            )
        } else {
            (
                to_single_quoted,
                "Prefer single-quoted strings unless the string contains a single quote.",
            )
        };
        let source = ctx.source;
        for token in source.code_tokens() {
            if token.kind != (TokenKind::String { long: false }) {
                continue;
            }
            if let Some(replacement) = convert(source.text_of(token)) {
                ctx.add_offense_with_fix(
                    token.range(),
                    message,
                    vec![Edit::replace(token.range(), replacement)],
                );
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(StringLiterals)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/StringLiterals";
    const SINGLE: &str = "Prefer single-quoted strings unless the string contains a single quote.";
    const DOUBLE: &str = "Prefer double-quoted strings unless the string contains a double quote.";

    #[test]
    fn flags_double_quotes() {
        expect_offense(READER, "x = \"abc\"\n", 1, 5, SINGLE);
        expect_offense(READER, "f(\"\")\n", 1, 3, SINGLE);
        expect_offense(READER, "x = \"say \\\"hi\\\"\"\n", 1, 5, SINGLE);
    }

    #[test]
    fn accepts_single_quotes_and_needed_double_quotes() {
        expect_no_offenses(READER, "x = 'abc'\ny = \"it's\"\nz = \"it\\'s\"\n");
        expect_no_offenses(READER, "x = [[\"abc\"]]\ny = [==[a]==]\n-- \"comment\"\n");
        expect_no_offenses(READER, "x = \"unterminated\n");
    }

    #[test]
    fn autocorrects_to_single_quotes() {
        expect_correction(READER, "x = \"abc\"\n", "x = 'abc'\n");
        expect_correction(READER, "x = \"say \\\"hi\\\"\"\n", "x = 'say \"hi\"'\n");
        expect_correction(READER, "x = \"a\\\\b\\n\"\n", "x = 'a\\\\b\\n'\n");
        expect_correction(READER, "x = \"it's\"..\"ok\"\n", "x = \"it's\"..'ok'\n");
    }

    #[test]
    fn double_quotes_style() {
        let offenses = inspect_with(
            READER,
            "x = 'abc'\ny = \"abc\"\n",
            "EnforcedStyle: double_quotes",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            (offenses[0].line, offenses[0].message.as_str()),
            (1, DOUBLE)
        );
        assert!(
            inspect_with(READER, "x = 'say \"hi\"'\n", "EnforcedStyle: double_quotes").is_empty()
        );
        assert_eq!(
            autocorrect_with(READER, "x = 'it\\'s'\n", "EnforcedStyle: double_quotes"),
            "x = \"it's\"\n"
        );
    }
}
