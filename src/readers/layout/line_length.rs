//! `Layout/LineLength`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};

/// Flags lines longer than `Max` characters.
///
/// With `AllowURI`, a line passes when its overrun is a URI that starts before the limit and
/// runs to the end of the line. With `IgnoreComments`, a line passes when its overrun starts
/// inside a comment.
pub struct LineLength;

impl Reader for LineLength {
    fn name(&self) -> &'static str {
        "Layout/LineLength"
    }

    fn description(&self) -> &'static str {
        "Checks that lines are not longer than the configured maximum."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("Max", Yaml::Integer(120)),
            ("AllowURI", Yaml::Boolean(true)),
            ("IgnoreComments", Yaml::Boolean(false)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let max = ctx.option_usize("Max", 120);
        let allow_uri = ctx.option_bool("AllowURI", true);
        let ignore_comments = ctx.option_bool("IgnoreComments", false);
        let source = ctx.source;
        for (number, line) in source.lines() {
            let content = line.strip_suffix('\r').unwrap_or(line);
            let length = content.chars().count();
            let Some((excess, _)) = content.char_indices().nth(max) else {
                continue;
            };
            let line_start = source.line_range(number).start;
            let start = line_start + excess;
            if ignore_comments
                && source
                    .token_at(start)
                    .is_some_and(|token| token.kind.is_comment())
            {
                continue;
            }
            if allow_uri && uri_overrun(content, max) {
                continue;
            }
            ctx.add_offense(
                start..line_start + content.len(),
                format!("Line is too long. [{length}/{max}]"),
            );
        }
    }
}

/// Whether the last URI on `line` starts within the first `max` characters and, together with
/// any punctuation glued to it, reaches the end of the line.
fn uri_overrun(line: &str, max: usize) -> bool {
    let Some(start) = last_uri_start(line) else {
        return false;
    };
    line[..start].chars().count() < max && !line[start..].contains(char::is_whitespace)
}

/// Byte offset of the scheme of the last `scheme://` URI on `line`.
fn last_uri_start(line: &str) -> Option<usize> {
    line.match_indices("://")
        .filter_map(|(index, _)| {
            let before = &line[..index];
            let start = before
                .char_indices()
                .rev()
                .find(|&(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
                .map_or(0, |(position, c)| position + c.len_utf8());
            before[start..]
                .starts_with(|c: char| c.is_ascii_alphabetic())
                .then_some(start)
        })
        .last()
}

inventory::submit! { Registration(|| Box::new(LineLength)) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const READER: &str = "Layout/LineLength";

    #[test]
    fn flags_long_lines() {
        let line = format!("x = '{}'\n", "a".repeat(120));
        expect_offense(READER, &line, 1, 121, "Line is too long. [126/120]");
    }

    #[test]
    fn accepts_lines_at_the_limit() {
        expect_no_offenses(READER, &format!("{}\n", "a".repeat(120)));
        expect_no_offenses(READER, "");
    }

    #[test]
    fn counts_characters_not_bytes() {
        expect_no_offenses(READER, &format!("x = '{}'\n", "é".repeat(114)));
        let offenses = inspect(READER, &format!("x = '{}'\n", "é".repeat(115)));
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, "Line is too long. [121/120]");
    }

    #[test]
    fn honours_max() {
        expect_offense_with_max();
        expect_no_offenses(READER, "local a = 1\n");
    }

    fn expect_offense_with_max() {
        let offenses = inspect_with(READER, "local abc = 1\n", "Max: 10");
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 11));
        assert_eq!(offenses[0].message, "Line is too long. [13/10]");
        assert_eq!(offenses[0].range.len(), 3);
        assert!(!offenses[0].correctable());
    }

    #[test]
    fn allows_trailing_uris() {
        let url = format!("-- see https://example.com/{}", "x".repeat(120));
        expect_no_offenses(READER, &url);
        expect_no_offenses(READER, &format!("local u = 'https://example.com/{}'", "x".repeat(120)));
        assert_eq!(inspect_with(READER, &url, "AllowURI: false").len(), 1);
        let followed = format!("{url} and more words");
        assert_eq!(inspect(READER, &followed).len(), 1);
        let late = format!("{} https://example.com", "x".repeat(130));
        assert_eq!(inspect(READER, &late).len(), 1);
    }

    #[test]
    fn can_ignore_comments() {
        let comment = format!("-- {}\n", "a".repeat(130));
        assert_eq!(inspect(READER, &comment).len(), 1);
        assert!(inspect_with(READER, &comment, "IgnoreComments: true").is_empty());
        let trailing = format!("x = 1 -- {}\n", "a".repeat(130));
        assert!(inspect_with(READER, &trailing, "IgnoreComments: true").is_empty());
        let code = format!("x = '{}' -- c\n", "a".repeat(130));
        assert_eq!(inspect_with(READER, &code, "IgnoreComments: true").len(), 1);
    }

    #[test]
    fn finds_uri_schemes() {
        assert_eq!(last_uri_start("a http://x b https://y"), Some(13));
        assert_eq!(last_uri_start("é://x"), None);
        assert_eq!(last_uri_start("'ftp://x'"), Some(1));
        assert_eq!(last_uri_start("no uri here"), None);
    }

    #[test]
    fn never_autocorrects() {
        let line = format!("{}\n", "a".repeat(130));
        assert_eq!(autocorrect(READER, &line), line);
    }
}
