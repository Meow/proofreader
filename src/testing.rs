//! Helpers for reader tests: run a single reader over a snippet with the default configuration
//! (optionally with extra options) and assert on the result.

use std::path::Path;

use crate::config::Config;
use crate::offense::Offense;
use crate::reader::reader_named;
use crate::runner::{Options, READER_ERROR, ReaderFilter, inspect_source};
use crate::source::Source;

/// Resolves a reader name, panicking with a helpful message when it is not registered.
fn registered(reader: &str) -> &'static str {
    match reader_named(reader) {
        Some(found) => found.name(),
        None => panic!("no reader named {reader} is registered"),
    }
}

/// A configuration with `options_yaml` (a YAML mapping of options) set for `reader`.
fn config_for(reader: &str, options_yaml: &str) -> Config {
    let yaml = if options_yaml.trim().is_empty() {
        String::new()
    } else {
        let indented: String = options_yaml
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect();
        format!("{reader}:\n{indented}")
    };
    match Config::from_yaml_str(&yaml, Path::new(".")) {
        Ok(config) => config,
        Err(error) => panic!("invalid options for {reader}: {error}"),
    }
}

/// Runs only `reader` over `src`, with or without autocorrection.
fn run(reader: &str, src: &str, options_yaml: &str, fix: bool) -> (Vec<Offense>, Option<String>) {
    let name = registered(reader);
    let config = config_for(name, options_yaml);
    let options = Options {
        fix,
        filter: ReaderFilter::single(name),
        ..Options::default()
    };
    let (offenses, corrected) = inspect_source(&Source::new("test.lua", src), &config, &options);
    if let Some(error) = offenses
        .iter()
        .find(|offense| offense.reader == READER_ERROR)
    {
        panic!("{}", error.message);
    }
    (offenses, corrected)
}

/// Offenses `reader` reports for `src` with the default configuration.
pub fn inspect(reader: &str, src: &str) -> Vec<Offense> {
    inspect_with(reader, src, "")
}

/// Offenses `reader` reports for `src` with `options_yaml` (for example `"Max: 80"`) applied.
pub fn inspect_with(reader: &str, src: &str, options_yaml: &str) -> Vec<Offense> {
    run(reader, src, options_yaml, false).0
}

/// `src` after running the autocorrect loop with only `reader`.
pub fn autocorrect(reader: &str, src: &str) -> String {
    autocorrect_with(reader, src, "")
}

/// `src` after running the autocorrect loop with only `reader` and `options_yaml` applied.
pub fn autocorrect_with(reader: &str, src: &str, options_yaml: &str) -> String {
    run(reader, src, options_yaml, true)
        .1
        .unwrap_or_else(|| src.to_owned())
}

/// Renders offenses as `line:col: message` lines for assertion messages.
fn describe(offenses: &[Offense]) -> String {
    offenses
        .iter()
        .map(|offense| format!("  {}:{}: {}\n", offense.line, offense.col, offense.message))
        .collect()
}

/// Asserts that `reader` reports nothing for `src`.
pub fn expect_no_offenses(reader: &str, src: &str) {
    let offenses = inspect(reader, src);
    assert!(
        offenses.is_empty(),
        "expected no {reader} offenses in {src:?}, got:\n{}",
        describe(&offenses)
    );
}

/// Asserts that `reader` reports exactly one offense for `src`, at `line`:`col` with `message`.
pub fn expect_offense(reader: &str, src: &str, line: u32, col: u32, message: &str) {
    expect_offenses(reader, src, &[(line, col, message)]);
}

/// Asserts that `reader` reports exactly `expected` (`(line, col, message)`, in order) for `src`.
pub fn expect_offenses(reader: &str, src: &str, expected: &[(u32, u32, &str)]) {
    let offenses = inspect(reader, src);
    let actual: Vec<(u32, u32, &str)> = offenses
        .iter()
        .map(|offense| (offense.line, offense.col, offense.message.as_str()))
        .collect();
    assert_eq!(
        actual,
        expected,
        "unexpected {reader} offenses in {src:?}:\n{}",
        describe(&offenses)
    );
}

/// Asserts that autocorrecting `src` with `reader` yields `expected`, and that correcting the
/// result again changes nothing.
pub fn expect_correction(reader: &str, src: &str, expected: &str) {
    let corrected = autocorrect(reader, src);
    assert_eq!(corrected, expected, "{reader} correction of {src:?}");
    assert_eq!(
        autocorrect(reader, &corrected),
        corrected,
        "{reader} correction is not idempotent"
    );
}
