//! `Layout/EndOfLine`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;

/// Checks the line endings: LF with `EnforcedStyle: lf` (the default), CRLF with `crlf`.
///
/// Reported once per file, at the first wrong line ending. With `lf` the fix drops the `\r` of
/// every CRLF pair and turns lone carriage returns (which Lua treats as line breaks) into `\n`;
/// with `crlf` it adds a `\r` before every `\n` that lacks one and a `\n` after every lone
/// `\r`. Line endings inside Lumen markup tokens (quoted attribute values and `<!-- -->`
/// comments spanning lines) are reported but never changed. Lua reads a CRLF inside a long
/// string as a single `\n`, so changing line endings never changes a string's value.
pub struct EndOfLine;

impl Reader for EndOfLine {
    fn name(&self) -> &'static str {
        "Layout/EndOfLine"
    }

    fn description(&self) -> &'static str {
        "Checks for consistent LF (or CRLF) line endings."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("lf".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        if ctx.option_str("EnforcedStyle", "lf") == "crlf" {
            check_crlf(ctx);
            return;
        }
        let source = ctx.source;
        let text = source.text.as_str();
        let bytes = text.as_bytes();
        let Some(first) = text.find('\r') else {
            return;
        };
        let edits: Vec<Edit> = text
            .match_indices('\r')
            .filter(|&(index, _)| !source.in_markup(index))
            .map(|(index, _)| {
                if bytes.get(index + 1) == Some(&b'\n') {
                    Edit::remove(index..index + 1)
                } else {
                    Edit::replace(index..index + 1, "\n")
                }
            })
            .collect();
        let offense = ctx.add_offense(first..first + 1, "Carriage return character detected.");
        if !edits.is_empty() {
            offense.with_fix(edits);
        }
    }
}

/// Reports the first line ending that is not CRLF, with a fix for all of them.
fn check_crlf(ctx: &mut Context) {
    let source = ctx.source;
    let bad = bare_line_endings(source);
    let Some(&(first, _)) = bad.first() else {
        return;
    };
    let edits: Vec<Edit> = bad
        .iter()
        .filter(|&&(index, _)| !source.in_markup(index))
        .map(|&(index, lone_return)| {
            if lone_return {
                Edit::insert(index + 1, "\n")
            } else {
                Edit::insert(index, "\r")
            }
        })
        .collect();
    let offense = ctx.add_offense(first..first + 1, "Carriage return character missing.");
    if !edits.is_empty() {
        offense.with_fix(edits);
    }
}

/// The line endings of `source` that are not CRLF, as `(byte offset, lone carriage return)`: a
/// `\n` without a `\r` before it, or a `\r` without a `\n` after it.
fn bare_line_endings(source: &Source) -> Vec<(usize, bool)> {
    let bytes = source.text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter_map(|(index, &byte)| match byte {
            b'\n' if index == 0 || bytes[index - 1] != b'\r' => Some((index, false)),
            b'\r' if bytes.get(index + 1) != Some(&b'\n') => Some((index, true)),
            _ => None,
        })
        .collect()
}

inventory::submit! { Registration(|| Box::new(EndOfLine)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EndOfLine";

    #[test]
    fn accepts_lf() {
        expect_no_offenses(READER, "a()\nb()\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_the_first_carriage_return_once() {
        expect_offense(
            READER,
            "a()\r\nb()\r\n",
            1,
            4,
            "Carriage return character detected.",
        );
        expect_offense(
            READER,
            "a()\nb()\r\n",
            2,
            4,
            "Carriage return character detected.",
        );
        expect_offense(
            READER,
            "a()\rb()\n",
            1,
            4,
            "Carriage return character detected.",
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "a()\r\nb()\r\n", "a()\nb()\n");
        expect_correction(READER, "s = [[x\r\ny]]\r\n", "s = [[x\ny]]\n");
        expect_correction(READER, "a()\rb()\r\n\r\n", "a()\nb()\n\n");
        expect_correction(READER, "a()\r\r\n", "a()\n\n");
    }

    #[test]
    fn crlf_style() {
        let options = "EnforcedStyle: crlf";
        assert!(inspect_with(READER, "a()\r\nb()\r\n", options).is_empty());
        assert!(inspect_with(READER, "", options).is_empty());
        let offenses = inspect_with(READER, "a()\r\nb()\nc()\n", options);
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            (
                offenses[0].line,
                offenses[0].col,
                offenses[0].message.as_str()
            ),
            (2, 4, "Carriage return character missing.")
        );
        assert_eq!(
            autocorrect_with(READER, "a()\nb()\rs = [[x\ny]]\n\n", options),
            "a()\r\nb()\r\ns = [[x\r\ny]]\r\n\r\n"
        );
        assert_eq!(autocorrect_with(READER, "\n\r\n", options), "\r\n\r\n");
    }

    #[test]
    fn crlf_style_never_changes_markup() {
        use crate::config::Config;
        use crate::runner::{Options, ReaderFilter, inspect_source};
        use crate::source::Source;
        use std::path::Path;

        let config =
            Config::from_yaml_str("Layout/EndOfLine:\n  EnforcedStyle: crlf\n", Path::new("."))
                .expect("config");
        let options = Options {
            fix: true,
            filter: ReaderFilter::single(READER),
            ..Options::default()
        };
        let src = "return <view title=\"a\nb\">\n</view>\n";
        let (offenses, corrected) = inspect_source(&Source::new("t.lumen", src), &config, &options);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 22));
        assert_eq!(
            corrected.as_deref(),
            Some("return <view title=\"a\nb\">\r\n</view>\r\n")
        );
    }

    #[test]
    fn never_changes_carriage_returns_inside_markup() {
        let src = "return <view title=\"a\r\nb\">\r\n</view>\r\n";
        expect_offense_lumen(READER, src, 1, 22, "Carriage return character detected.");
        expect_correction_lumen(READER, src, "return <view title=\"a\r\nb\">\n</view>\n");
        let markup_only = "return <view title=\"a\r\nb\"/>\n";
        expect_offense_lumen(
            READER,
            markup_only,
            1,
            22,
            "Carriage return character detected.",
        );
        assert_eq!(autocorrect_lumen(READER, markup_only), markup_only);
    }
}

#[cfg(test)]
mod corpus {
    use std::path::Path;

    use crate::config::Config;
    use crate::runner::{Options, inspect_source};
    use crate::source::Source;

    /// Every reader together, with `EnforcedStyle: crlf`, corrects CRLF copies of the Flux
    /// corpus into CRLF files that need no further correction.
    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
    fn crlf_corrections_converge_on_the_flux_corpus() {
        let root = Path::new("/home/luna/code/flux-ce");
        let config = Config::from_yaml_str("Layout/EndOfLine:\n  EnforcedStyle: crlf\n", root)
            .expect("config");
        let lf = Config::from_yaml_str("", root).expect("config");
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let mut differing = Vec::new();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "lua" || ext == "lumen")
            })
        {
            let path = entry.path();
            let text = std::fs::read_to_string(path).expect("UTF-8 source");
            let lf_corrected = inspect_source(&Source::new(path, text.clone()), &lf, &options)
                .1
                .unwrap_or_else(|| text.clone());
            let crlf_corrected = inspect_source(
                &Source::new(path, text.replace('\n', "\r\n")),
                &config,
                &options,
            )
            .1
            .unwrap_or_else(|| text.replace('\n', "\r\n"));
            if crlf_corrected.replace("\r\n", "\n") != lf_corrected {
                differing.push(path.display().to_string());
            }
            for input in [text.replace('\n', "\r\n"), text.clone()] {
                let (_, corrected) =
                    inspect_source(&Source::new(path, input.clone()), &config, &options);
                let corrected = corrected.unwrap_or(input);
                let (offenses, again) =
                    inspect_source(&Source::new(path, corrected.clone()), &config, &options);
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
                let source = Source::new(path, corrected.clone());
                let bare = corrected
                    .match_indices('\n')
                    .filter(|&(index, _)| {
                        !source.in_markup(index)
                            && (index == 0 || corrected.as_bytes()[index - 1] != b'\r')
                    })
                    .count();
                assert_eq!(bare, 0, "{} keeps LF line endings", path.display());
            }
        }
        assert!(
            differing.is_empty(),
            "CRLF corrections differ: {differing:?}"
        );
    }
}
