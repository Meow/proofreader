//! Helpers for reader tests: run a single reader over a snippet with the default configuration
//! (optionally with extra options) and assert on the result.
//!
//! Snippets are GLua (`test.lua`); the `_lumen` variants treat them as Lumen templates
//! (`test.lumen`).

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
    config_from(reader, &yaml)
}

/// The configuration `yaml` (a whole `.proofreader.yml`), which must be valid.
fn config_from(reader: &str, yaml: &str) -> Config {
    match Config::from_yaml_str(yaml, Path::new(".")) {
        Ok(config) => config,
        Err(error) => panic!("invalid options for {reader}: {error}"),
    }
}

/// The file GLua snippets are inspected as.
const LUA: &str = "test.lua";

/// The file Lumen snippets are inspected as.
const LUMEN: &str = "test.lumen";

/// Runs only `reader` over `src` inspected as the file `path`, with or without autocorrection.
fn run(
    path: &str,
    reader: &str,
    src: &str,
    options_yaml: &str,
    fix: bool,
) -> (Vec<Offense>, Option<String>) {
    let name = registered(reader);
    let config = config_for(name, options_yaml);
    run_with_config(path, name, src, &config, fix)
}

/// Runs only `reader` over `src` inspected as the file `path` under `config`.
fn run_with_config(
    path: &str,
    name: &'static str,
    src: &str,
    config: &Config,
    fix: bool,
) -> (Vec<Offense>, Option<String>) {
    let options = Options {
        fix,
        filter: ReaderFilter::single(name),
        ..Options::default()
    };
    let (offenses, corrected) = inspect_source(&Source::new(path, src), config, &options);
    if let Some(error) = offenses
        .iter()
        .find(|offense| offense.reader == READER_ERROR)
    {
        panic!("{}", error.message);
    }
    (offenses, corrected)
}

/// `src` inspected as the file `path` after running the autocorrect loop with only `reader`.
fn correct(path: &str, reader: &str, src: &str, options_yaml: &str) -> String {
    run(path, reader, src, options_yaml, true)
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

/// Asserts that `reader` reports nothing for `src` inspected as the file `path`.
fn assert_no_offenses(path: &str, reader: &str, src: &str) {
    let offenses = run(path, reader, src, "", false).0;
    assert!(
        offenses.is_empty(),
        "expected no {reader} offenses in {path} {src:?}, got:\n{}",
        describe(&offenses)
    );
}

/// Asserts that `reader` reports exactly `expected` (`(line, col, message)`, in order) for `src`
/// inspected as the file `path`.
fn assert_offenses(path: &str, reader: &str, src: &str, expected: &[(u32, u32, &str)]) {
    let offenses = run(path, reader, src, "", false).0;
    let actual: Vec<(u32, u32, &str)> = offenses
        .iter()
        .map(|offense| (offense.line, offense.col, offense.message.as_str()))
        .collect();
    assert_eq!(
        actual,
        expected,
        "unexpected {reader} offenses in {path} {src:?}:\n{}",
        describe(&offenses)
    );
}

/// Asserts that autocorrecting `src`, inspected as the file `path`, with `reader` yields
/// `expected`, and that correcting the result again changes nothing.
fn assert_correction(path: &str, reader: &str, src: &str, expected: &str) {
    let corrected = correct(path, reader, src, "");
    assert_eq!(corrected, expected, "{reader} correction of {path} {src:?}");
    assert_eq!(
        correct(path, reader, &corrected, ""),
        corrected,
        "{reader} correction of {path} is not idempotent"
    );
}

/// Offenses `reader` reports for `src` under the whole configuration file `config_yaml`
/// (for settings outside the reader's own section, such as a department section).
pub fn inspect_configured(reader: &str, src: &str, config_yaml: &str) -> Vec<Offense> {
    let name = registered(reader);
    let config = config_from(name, config_yaml);
    run_with_config(LUA, name, src, &config, false).0
}

/// Offenses `reader` reports for `src` with the default configuration.
pub fn inspect(reader: &str, src: &str) -> Vec<Offense> {
    inspect_with(reader, src, "")
}

/// Offenses `reader` reports for `src` with `options_yaml` (for example `"Max: 80"`) applied.
pub fn inspect_with(reader: &str, src: &str, options_yaml: &str) -> Vec<Offense> {
    run(LUA, reader, src, options_yaml, false).0
}

/// `src` after running the autocorrect loop with only `reader`.
pub fn autocorrect(reader: &str, src: &str) -> String {
    autocorrect_with(reader, src, "")
}

/// `src` after running the autocorrect loop with only `reader` and `options_yaml` applied.
pub fn autocorrect_with(reader: &str, src: &str, options_yaml: &str) -> String {
    correct(LUA, reader, src, options_yaml)
}

/// Asserts that `reader` reports nothing for `src`.
pub fn expect_no_offenses(reader: &str, src: &str) {
    assert_no_offenses(LUA, reader, src);
}

/// Asserts that `reader` reports exactly one offense for `src`, at `line`:`col` with `message`.
pub fn expect_offense(reader: &str, src: &str, line: u32, col: u32, message: &str) {
    expect_offenses(reader, src, &[(line, col, message)]);
}

/// Asserts that `reader` reports exactly `expected` (`(line, col, message)`, in order) for `src`.
pub fn expect_offenses(reader: &str, src: &str, expected: &[(u32, u32, &str)]) {
    assert_offenses(LUA, reader, src, expected);
}

/// Asserts that autocorrecting `src` with `reader` yields `expected`, and that correcting the
/// result again changes nothing.
pub fn expect_correction(reader: &str, src: &str, expected: &str) {
    assert_correction(LUA, reader, src, expected);
}

/// Offenses `reader` reports for the Lumen template `src` with the default configuration.
pub fn inspect_lumen(reader: &str, src: &str) -> Vec<Offense> {
    run(LUMEN, reader, src, "", false).0
}

/// The Lumen template `src` after running the autocorrect loop with only `reader`.
pub fn autocorrect_lumen(reader: &str, src: &str) -> String {
    correct(LUMEN, reader, src, "")
}

/// Asserts that `reader` reports nothing for the Lumen template `src`.
pub fn expect_no_offenses_lumen(reader: &str, src: &str) {
    assert_no_offenses(LUMEN, reader, src);
}

/// Asserts that `reader` reports exactly one offense for the Lumen template `src`, at
/// `line`:`col` with `message`.
pub fn expect_offense_lumen(reader: &str, src: &str, line: u32, col: u32, message: &str) {
    expect_offenses_lumen(reader, src, &[(line, col, message)]);
}

/// Asserts that `reader` reports exactly `expected` (`(line, col, message)`, in order) for the
/// Lumen template `src`.
pub fn expect_offenses_lumen(reader: &str, src: &str, expected: &[(u32, u32, &str)]) {
    assert_offenses(LUMEN, reader, src, expected);
}

/// Asserts that autocorrecting the Lumen template `src` with `reader` yields `expected`, and
/// that correcting the result again changes nothing.
pub fn expect_correction_lumen(reader: &str, src: &str, expected: &str) {
    assert_correction(LUMEN, reader, src, expected);
}
