//! `Layout/LineLength`.

use yaml_rust2::Yaml;

use super::line_breaking::LineBreaker;
use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;

/// Flags lines longer than `Max` characters and breaks overlong code lines.
///
/// With `AllowURI`, a line passes when its overrun is a URI that starts before the limit and
/// runs to the end of the line. With `IgnoreComments`, a line passes when its overrun starts
/// inside a comment.
///
/// The fix only changes whitespace (see [`super::line_breaking`]): it breaks after the `=` of
/// an assignment, unfolds an argument list or table constructor to one item per line, or breaks
/// a `..`/`and`/`or`/`,` chain or a method chain, indenting new lines by `IndentationWidth`.
/// Comment-only lines, lines that overflow inside a trailing comment and lines without a usable
/// break point (a single long string, alignment padding) are not corrected.
pub struct LineLength;

/// The options of `Layout/LineLength` that decide which lines are too long and how deep new
/// lines are indented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `Max`: the maximum number of characters per line.
    pub max: usize,
    /// `AllowURI`: whether a URI running to the end of the line may overrun the maximum.
    pub allow_uri: bool,
    /// `IgnoreComments`: whether an overrun starting inside a comment is accepted.
    pub ignore_comments: bool,
    /// `IndentationWidth`: spaces per indentation level of the lines a fix inserts.
    pub width: usize,
}

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
            ("IndentationWidth", Yaml::Integer(2)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let limits = Limits {
            max: ctx.option_usize("Max", 120),
            allow_uri: ctx.option_bool("AllowURI", true),
            ignore_comments: ctx.option_bool("IgnoreComments", false),
            width: ctx.option_usize("IndentationWidth", 2),
        };
        let correct = ctx.config.autocorrect;
        let source = ctx.source;
        let mut breaker = None;
        for number in 1..=source.line_count() {
            let Some(start) = overflow(source, number, &limits) else {
                continue;
            };
            let line = source.line(number);
            let content = line.strip_suffix('\r').unwrap_or(line);
            let end = source.line_range(number).start + content.len();
            let fix = if correct {
                breaker
                    .get_or_insert_with(|| LineBreaker::new(source, limits))
                    .plan(number)
            } else {
                None
            };
            let offense = ctx.add_offense(
                start..end,
                format!(
                    "Line is too long. [{}/{}]",
                    content.chars().count(),
                    limits.max
                ),
            );
            if let Some(rewrite) = fix {
                offense.with_fix(vec![Edit::replace(rewrite.range, rewrite.text)]);
            }
        }
    }
}

/// The byte offset where line `n` overruns `limits.max`, unless the line is shorter or its
/// overrun is accepted by `AllowURI` or `IgnoreComments`.
pub fn overflow(source: &Source, n: u32, limits: &Limits) -> Option<usize> {
    let line = source.line(n);
    let content = line.strip_suffix('\r').unwrap_or(line);
    let (excess, _) = content.char_indices().nth(limits.max)?;
    let start = source.line_range(n).start + excess;
    if limits.ignore_comments
        && source
            .token_at(start)
            .is_some_and(|token| token.kind.is_comment())
    {
        return None;
    }
    if limits.allow_uri && uri_overrun(content, limits.max) {
        return None;
    }
    Some(start)
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
        expect_no_offenses(
            READER,
            &format!("local u = 'https://example.com/{}'", "x".repeat(120)),
        );
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
    fn breaks_after_the_equals_sign() {
        expect_correction_with(
            "local a = aaaaaaaaaaaaaaaaaaaaaaaaaaaa()\n",
            "Max: 35",
            "local a =\n  aaaaaaaaaaaaaaaaaaaaaaaaaaaa()\n",
        );
        expect_correction_with(
            "function f()\n  self.value = compute_something(1)\nend\n",
            "Max: 30",
            "function f()\n  self.value =\n    compute_something(1)\nend\n",
        );
    }

    #[test]
    fn breaks_table_fields_after_the_equals_sign() {
        expect_correction_with(
            "local t = {\n  name = compute_the_name(1),\n  other = 2\n}\n",
            "Max: 25",
            "local t = {\n  name =\n    compute_the_name(1),\n  other = 2\n}\n",
        );
    }

    #[test]
    fn unfolds_argument_lists() {
        expect_correction_with(
            "function_call(aaaaaaaaaa, bbbbbbbbbbbbb, cccccccccc, dddddddd)\n",
            "Max: 40",
            "function_call(\n  aaaaaaaaaa,\n  bbbbbbbbbbbbb,\n  cccccccccc,\n  dddddddd\n)\n",
        );
        expect_correction_with(
            "if x then\n  draw.Text(label, font, pos_x, pos_y, color)\nend\n",
            "Max: 40",
            "if x then\n  draw.Text(\n    label,\n    font,\n    pos_x,\n    pos_y,\n    color\n  )\nend\n",
        );
    }

    #[test]
    fn unfolds_the_outermost_list_holding_the_overrun() {
        expect_correction_with(
            "print(format_name(first_name, last_name), age)\n",
            "Max: 40",
            "print(\n  format_name(first_name, last_name),\n  age\n)\n",
        );
        expect_correction_with(
            "outer(inner(first_argument, second_argument))\n",
            "Max: 30",
            "outer(\n  inner(\n    first_argument,\n    second_argument\n  )\n)\n",
        );
    }

    #[test]
    fn unfolds_table_constructors() {
        expect_correction_with(
            "configure({ alpha = 1, beta = 2, gamma = 3 })\n",
            "Max: 30",
            "configure({\n  alpha = 1,\n  beta = 2,\n  gamma = 3\n})\n",
        );
        expect_correction_with(
            "notify('message', { target = actor, value = 10 })\n",
            "Max: 40",
            "notify('message', {\n  target = actor,\n  value = 10\n})\n",
        );
        expect_correction_with(
            "connect { host = settings.host, port = settings.port }\n",
            "Max: 40",
            "connect {\n  host = settings.host,\n  port = settings.port\n}\n",
        );
    }

    #[test]
    fn unfolds_lists_ending_in_a_multiline_function() {
        expect_correction_with(
            "hook.Add('PlayerSpawn', 'my_unique_identifier', function(client)\n  client:SetHealth(100)\nend)\n",
            "Max: 50",
            "hook.Add(\n  'PlayerSpawn',\n  'my_unique_identifier',\n  function(client)\n    client:SetHealth(100)\n  end\n)\n",
        );
        expect_correction_with(
            "timer.Simple(1, function()\n  run(first, second, function(result)\n    use(result)\n  end)\nend)\n",
            "Max: 35",
            "timer.Simple(1, function()\n  run(\n    first,\n    second,\n    function(result)\n      use(result)\n    end\n  )\nend)\n",
        );
    }

    #[test]
    fn unfolds_only_the_items_of_a_list_already_on_several_lines() {
        expect_correction_with(
            "foo(alpha, beta, gamma,\n    delta)\n",
            "Max: 20",
            "foo(\n  alpha,\n  beta,\n  gamma,\n  delta\n)\n",
        );
        expect_correction_with(
            "request(title, message, '',\nfunction(text)\n  use(text)\nend, nil, label)\n",
            "Max: 25",
            "request(\n  title,\n  message,\n  '',\n  function(text)\n    use(text)\n  end, nil, label\n)\n",
        );
    }

    #[test]
    fn breaks_chains_after_operators() {
        expect_correction_with(
            "return first_part..second_part..third_part\n",
            "Max: 40",
            "return first_part..second_part..\n  third_part\n",
        );
        expect_correction_with(
            "return alpha_value or beta_value or gamma_value\n",
            "Max: 40",
            "return alpha_value or beta_value or\n  gamma_value\n",
        );
        expect_correction_with(
            "return first_value, second_value, third_value\n",
            "Max: 40",
            "return first_value, second_value,\n  third_value\n",
        );
    }

    #[test]
    fn breaks_at_the_loosest_operator_first() {
        expect_correction_with(
            "return alpha and beta..gamma or delta..epsilon\n",
            "Max: 35",
            "return alpha and beta..gamma or\n  delta..epsilon\n",
        );
    }

    #[test]
    fn keeps_operators_first_when_the_chain_does() {
        expect_correction_with(
            "local text = 'a'\n  ..first_part..second_part..third_part\n",
            "Max: 30",
            "local text = 'a'\n  ..first_part..second_part\n  ..third_part\n",
        );
    }

    #[test]
    fn aligns_conditions_under_the_header() {
        expect_correction_with(
            "if alpha_value and beta_value and gamma_value then\n  run()\nend\n",
            "Max: 40",
            "if alpha_value and beta_value and\n   gamma_value then\n  run()\nend\n",
        );
        expect_correction_with(
            "if a then\n  b()\nelseif alpha_value or beta_value or gamma then\n  c()\nend\n",
            "Max: 40",
            "if a then\n  b()\nelseif alpha_value or beta_value or\n       gamma then\n  c()\nend\n",
        );
        expect_correction_with(
            "while alpha_value and beta_value and gamma_value do\n  run()\nend\n",
            "Max: 40",
            "while alpha_value and beta_value and\n      gamma_value do\n  run()\nend\n",
        );
    }

    #[test]
    fn breaks_guard_clauses_only_where_no_blank_line_is_needed() {
        expect_correction_with(
            "function f()\n  if !alpha_value or !beta_value then return end\n\n  run()\nend\n",
            "Max: 40",
            "function f()\n  if !alpha_value or\n     !beta_value then return end\n\n  run()\nend\n",
        );
        let guarded = "function f()\n  run()\n  if !alpha_value or !beta_value then return end\n\n  run()\nend\n";
        expect_no_correction(guarded, "Max: 40");
    }

    #[test]
    fn breaks_method_chains_before_the_colon() {
        expect_correction_with(
            "object:first_method():second_method():third()\n",
            "Max: 40",
            "object:first_method():second_method()\n  :third()\n",
        );
    }

    #[test]
    fn breaks_a_line_in_several_passes() {
        expect_correction_with(
            "local result = combine(first_long_name..second_long_name..third_long_name, other)\n",
            "Max: 50",
            "local result = combine(\n  first_long_name..second_long_name..\n    third_long_name,\n  other\n)\n",
        );
    }

    #[test]
    fn honours_the_indentation_width() {
        let corrected = autocorrect_with(
            READER,
            "function_call(aaaaaaaaaa, bbbbbbbbbbbbb)\n",
            "Max: 30\nIndentationWidth: 4",
        );
        assert_eq!(
            corrected,
            "function_call(\n    aaaaaaaaaa,\n    bbbbbbbbbbbbb\n)\n"
        );
    }

    #[test]
    fn keeps_trailing_comments_after_their_code() {
        expect_correction_with(
            "call_something(first_argument, second) -- note\n",
            "Max: 35",
            "call_something(\n  first_argument,\n  second\n) -- note\n",
        );
        expect_no_correction("x = f(a, b) -- a comment that is far too long\n", "Max: 30");
    }

    #[test]
    fn leaves_unbreakable_lines_alone() {
        expect_no_correction(&format!("local message = '{}'\n", "x".repeat(120)), "");
        expect_no_correction(&format!("-- {}\n", "a".repeat(130)), "");
        expect_no_correction(&format!("{}\n", "a".repeat(130)), "");
        expect_no_correction("local x = [[\nsome long text in a string]]\n", "Max: 20");
        expect_no_correction("local value = alpha * beta - gamma\n", "Max: 20");
        expect_no_correction("x = f(1)\nlocal t = { a,  b, c }\n", "Max: 15");
        expect_no_correction("for key, value in pairs(some_table) do\nend\n", "Max: 20");
    }

    #[test]
    fn keeps_alignment_of_neighbouring_lines() {
        expect_no_correction(
            "local short = call(a,     beta)\nlocal other = call(alpha, beta)\n",
            "Max: 30",
        );
        expect_correction_with(
            "local short = call(a, beta)\nlocal other = call(alpha, beta)\n",
            "Max: 30",
            "local short = call(a, beta)\nlocal other =\n  call(alpha, beta)\n",
        );
        expect_correction_with(
            "local ab   = 1\nlocal cdef = f(alpha, beta)\n",
            "Max: 20",
            "local ab   = 1\nlocal cdef =\n  f(alpha, beta)\n",
        );
    }

    #[test]
    fn can_be_disabled() {
        let line = "function_call(aaaaaaaaaa, bbbbbbbbbbbbb)\n";
        let offenses = inspect_with(READER, line, "Max: 30\nAutoCorrect: false");
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
        assert!(inspect_with(READER, line, "Max: 30")[0].correctable());
    }

    #[test]
    fn corrections_add_no_offenses_of_other_readers() {
        let samples = [
            ("local a = aaaaaaaaaaaaaaaaaaaaaaaaaaaa()\n", "Max: 35"),
            (
                "function_call(aaaaaaaaaa, bbbbbbbbbbbbb, cccccccccc, dddddddd)\n",
                "Max: 40",
            ),
            (
                "local t = {\n  name = compute_the_name(1),\n  other = 2\n}\n",
                "Max: 25",
            ),
            ("configure({ alpha = 1, beta = 2, gamma = 3 })\n", "Max: 30"),
            (
                "x = 1\n\nhook.Add('PlayerSpawn', 'my_unique_identifier', function(client)\n  client:SetHealth(100)\nend)\n\ny = 2\n",
                "Max: 50",
            ),
            ("foo(alpha, beta, gamma,\n    delta)\n", "Max: 20"),
            (
                "if alpha_value and beta_value and gamma_value then\n  run()\nend\n",
                "Max: 40",
            ),
            (
                "function f()\n  if !alpha_value or !beta_value then return end\n\n  run()\nend\n",
                "Max: 40",
            ),
            ("object:first_method():second_method():third()\n", "Max: 40"),
            (
                "local text = 'a'\n  ..first_part..second_part..third_part\n",
                "Max: 30",
            ),
            (
                "local result = combine(first_long_name..second_long_name..third_long_name, other)\n",
                "Max: 50",
            ),
        ];
        for (src, options) in samples {
            let corrected = autocorrect_with(READER, src, options);
            assert_ne!(corrected, src, "{src:?} is not corrected");
            let before = other_offenses(src, options);
            let after = other_offenses(&corrected, options);
            for (reader, count) in &after {
                let previous = before.get(reader).copied().unwrap_or(0);
                assert!(
                    *count <= previous,
                    "{reader} reports {count} offenses instead of {previous} in {corrected:?}"
                );
            }
        }
    }

    /// Offense counts of every reader but `Layout/LineLength` for `src`, with `options` applied
    /// to `Layout/LineLength`.
    fn other_offenses(src: &str, options: &str) -> std::collections::HashMap<&'static str, usize> {
        let yaml = format!(
            "Layout/LineLength:\n{}",
            options
                .lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>()
        );
        let config =
            crate::config::Config::from_yaml_str(&yaml, std::path::Path::new(".")).expect("config");
        let (offenses, _) = crate::runner::inspect_source(
            &crate::source::Source::new("test.lua", src),
            &config,
            &crate::runner::Options::default(),
        );
        let mut counts = std::collections::HashMap::new();
        for offense in offenses.iter().filter(|offense| offense.reader != READER) {
            *counts.entry(offense.reader).or_insert(0) += 1;
        }
        counts
    }

    /// Asserts that correcting `src` with `options` yields `expected`, idempotently, and that
    /// the result has no overlong lines.
    fn expect_correction_with(src: &str, options: &str, expected: &str) {
        let corrected = autocorrect_with(READER, src, options);
        assert_eq!(corrected, expected, "correction of {src:?}");
        assert_eq!(
            autocorrect_with(READER, &corrected, options),
            corrected,
            "correction of {src:?} is not idempotent"
        );
        assert!(
            inspect_with(READER, &corrected, options).is_empty(),
            "{corrected:?} still has long lines"
        );
    }

    /// Asserts that `src` has a long line that is reported without a fix.
    fn expect_no_correction(src: &str, options: &str) {
        let offenses = inspect_with(READER, src, options);
        assert!(!offenses.is_empty(), "{src:?} has no long line");
        assert!(
            offenses.iter().all(|offense| !offense.correctable()),
            "{src:?} is correctable"
        );
        assert_eq!(autocorrect_with(READER, src, options), src);
    }
}
