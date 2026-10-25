//! `Layout/IndentationStyle`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::TokenKind;

/// Checks the characters of the leading indentation of a line.
///
/// With `EnforcedStyle: spaces` (the default) tabs are flagged and the fix replaces each tab with
/// `IndentationWidth` spaces. With `tabs` the indentation must be tabs for whole levels followed
/// by fewer than `IndentationWidth` spaces of alignment: its width (a tab counting up to the next
/// multiple of `IndentationWidth`) is written as `width / IndentationWidth` tabs and `width %
/// IndentationWidth` spaces, and the fix rewrites it that way.
///
/// Lines inside multi-line strings and markup tokens and whitespace-only lines (left to
/// `Layout/TrailingWhitespace`) are not checked.
///
/// `Layout/IndentationWidth` and `Layout/IndentationConsistency` measure a tab as reaching the
/// next multiple of their own width option and re-indent with spaces, which this reader then
/// turns into tabs; keep those widths equal to `IndentationWidth` when using tabs.
/// `Layout/LineLength` does not break lines indented with tabs.
pub struct IndentationStyle;

impl Reader for IndentationStyle {
    fn name(&self) -> &'static str {
        "Layout/IndentationStyle"
    }

    fn description(&self) -> &'static str {
        "Checks that indentation uses spaces, not tabs."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("EnforcedStyle", Yaml::String("spaces".to_owned())),
            ("IndentationWidth", Yaml::Integer(2)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let width = ctx.option_usize("IndentationWidth", 2).max(1);
        if ctx.option_str("EnforcedStyle", "spaces") == "tabs" {
            check_tabs(ctx, width);
            return;
        }
        let spaces = " ".repeat(width);
        let source = ctx.source;
        for (number, _) in source.lines() {
            let indentation = source.indentation(number);
            let (Some(first), Some(last)) = (indentation.find('\t'), indentation.rfind('\t'))
            else {
                continue;
            };
            if !checked(source, number) {
                continue;
            }
            let line_start = source.line_range(number).start;
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

/// Whether line `n` is checked: not blank and not starting inside a string or markup token.
fn checked(source: &Source, n: u32) -> bool {
    let line_start = source.line_range(n).start;
    !source.is_blank(n)
        && !source
            .token_at(line_start)
            .is_some_and(|token| token.kind.is_string() || token.kind == TokenKind::Markup)
}

/// Reports indentation that is not tabs followed by fewer than `width` spaces.
fn check_tabs(ctx: &mut Context, width: usize) {
    let source = ctx.source;
    for (number, _) in source.lines() {
        let indentation = source.indentation(number);
        if !indentation.contains(' ') || !checked(source, number) {
            continue;
        }
        let expected = tab_indentation(indentation, width);
        if expected == indentation {
            continue;
        }
        let line_start = source.line_range(number).start;
        let range = line_start..line_start + indentation.len();
        ctx.add_offense_with_fix(
            range.clone(),
            "Space detected in indentation.",
            vec![Edit::replace(range, expected)],
        );
    }
}

/// `indentation` rewritten as tabs for whole levels of `width` columns followed by spaces.
fn tab_indentation(indentation: &str, width: usize) -> String {
    let columns = indentation.chars().fold(0, |column, c| {
        if c == '\t' {
            (column / width + 1) * width
        } else {
            column + 1
        }
    });
    format!(
        "{}{}",
        "\t".repeat(columns / width),
        " ".repeat(columns % width)
    )
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

    #[test]
    fn tabs_style() {
        let options = "EnforcedStyle: tabs";
        assert!(
            inspect_with(
                READER,
                "if x then\n\ty()\n\t\tz()\n\t a()\nend\ns = [[\n    data\n]]\n  \n",
                options
            )
            .is_empty()
        );
        let offenses = inspect_with(READER, "do\n  x()\n \ty()\nend\n", options);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|o| (o.line, o.col, o.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (2, 1, "Space detected in indentation."),
                (3, 1, "Space detected in indentation."),
            ]
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "do\n  x()\n    y()\n     z()\n \ta()\n\t  b()\nend\n",
                options
            ),
            "do\n\tx()\n\t\ty()\n\t\t z()\n\ta()\n\t\tb()\nend\n"
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "do\n    x()\n      y()\nend\n",
                "EnforcedStyle: tabs\nIndentationWidth: 4"
            ),
            "do\n\tx()\n\t  y()\nend\n"
        );
    }

    #[test]
    fn checks_markup_lines_but_not_markup_tokens() {
        expect_correction_lumen(
            READER,
            "return <view>\n\t<text/>\n</view>\n",
            "return <view>\n  <text/>\n</view>\n",
        );
        expect_no_offenses_lumen(READER, "return <view title=\"a\n\tb\"/>\n");
    }
}

#[cfg(test)]
mod convergence {
    use std::path::Path;

    use crate::config::Config;
    use crate::runner::{Options, inspect_source};
    use crate::source::Source;

    const TABS: &str = "Layout/IndentationStyle:\n  EnforcedStyle: tabs\n";

    /// Corrects `text` with every reader under `config`, asserting that the result needs no
    /// further correction and keeps no correctable offense; returns it.
    fn converge(path: &Path, text: &str, config: &Config) -> String {
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let corrected = inspect_source(&Source::new(path, text), config, &options)
            .1
            .unwrap_or_else(|| text.to_owned());
        let (offenses, again) =
            inspect_source(&Source::new(path, corrected.clone()), config, &options);
        assert!(again.is_none(), "{} is not idempotent", path.display());
        let correctable: Vec<_> = offenses
            .iter()
            .filter(|offense| offense.correctable())
            .map(|offense| (offense.reader, offense.line, &offense.message))
            .collect();
        assert!(
            correctable.is_empty(),
            "{}: {correctable:?}",
            path.display()
        );
        corrected
    }

    #[test]
    fn every_reader_converges_with_tabs() {
        let config = Config::from_yaml_str(TABS, Path::new(".")).expect("config");
        let corrected = converge(
            Path::new("t.lua"),
            "local function f(a)\n    if a then\n      return {\n        x = 1\n      }\n    end\n  end\n\nreturn f\n",
            &config,
        );
        assert_eq!(
            corrected,
            "local function f(a)\n\tif a then\n\t\treturn {\n\t\t\tx = 1\n\t\t}\n\tend\nend\n\nreturn f\n"
        );
        let tabbed = "local function f(a)\n\tif a then\n\t\tx()\n\tend\nend\n\nreturn f\n";
        assert_eq!(converge(Path::new("t.lua"), tabbed, &config), tabbed);
    }

    /// Converting the Flux corpus to tabs and correcting tab-indented code converge.
    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce and Helix from /home/luna/code/helix"]
    fn tabs_corrections_converge_on_the_corpora() {
        for root in ["/home/luna/code/flux-ce", "/home/luna/code/helix"] {
            let root = Path::new(root);
            let config = Config::from_yaml_str(TABS, root).expect("config");
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lua"))
            {
                let path = entry.path();
                let Ok(text) = std::fs::read_to_string(path) else {
                    continue;
                };
                let corrected = converge(path, &text, &config);
                let source = Source::new(path, corrected.clone());
                let opaque = source.opaque_lines();
                for (number, _) in source.lines() {
                    if opaque.contains(&number) || source.is_blank(number) {
                        continue;
                    }
                    assert!(
                        !source.indentation(number).starts_with("  "),
                        "{}:{number} keeps space indentation",
                        path.display()
                    );
                }
            }
        }
    }
}
