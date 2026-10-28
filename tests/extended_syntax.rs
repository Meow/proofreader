//! Runs a corpus of tricky cases with the LuaJIT that accepts the syntax extensions of LuaJIT
//! 3.0, before and after the readers rewriting code into those extensions fix it, and after
//! `Lint/ExtendedSyntax` turns the result back into plain Lua: every version must print the
//! same lines.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use proofreader::config::Config;
use proofreader::offense::Offense;
use proofreader::runner::{Options, ReaderFilter, inspect_source};
use proofreader::source::Source;

/// The LuaJIT with the syntax extensions.
const LUAJIT: &str = "/home/luna/code/nl/LuaJIT/src/luajit";

/// The readers whose fixes write the syntax extensions.
const REWRITING_READERS: [&str; 5] = [
    "Style/BitOperators",
    "Style/CompoundAssignment",
    "Style/NumericSeparators",
    "Style/ShortFunction",
    "Style/TernaryOperator",
];

/// What `code` prints when LuaJIT runs it; panics when it fails.
fn run(code: &str) -> String {
    let mut child = Command::new(LUAJIT)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("LuaJIT runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(code.as_bytes())
        .expect("write the code");
    let output = child.wait_with_output().expect("LuaJIT finishes");
    assert!(
        output.status.success(),
        "LuaJIT failed: {}\n{code}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

/// `code` corrected by `readers` alone (enabled whatever their defaults), with the offenses
/// found on the way.
fn correct(code: &str, readers: &[&str]) -> (String, Vec<Offense>) {
    let yaml: String = readers
        .iter()
        .map(|reader| format!("{reader}:\n  Enabled: true\n"))
        .collect();
    let config = Config::from_yaml_str(&yaml, Path::new(".")).expect("configuration");
    let options = Options {
        fix: true,
        filter: ReaderFilter::new(
            &readers
                .iter()
                .map(|reader| (*reader).to_owned())
                .collect::<Vec<_>>(),
            &[],
        )
        .expect("known readers"),
        ..Options::default()
    };
    let (offenses, corrected) = inspect_source(&Source::new("corpus.lua", code), &config, &options);
    (corrected.unwrap_or_else(|| code.to_owned()), offenses)
}

#[test]
#[ignore = "runs the LuaJIT at /home/luna/code/nl/LuaJIT"]
fn rewrites_keep_what_the_corpus_prints() {
    let corpus = include_str!("extended/corpus.lua");
    let expected = run(corpus);
    let (extended, offenses) = correct(corpus, &REWRITING_READERS);
    for reader in REWRITING_READERS {
        assert!(
            offenses
                .iter()
                .any(|offense| offense.reader == reader && offense.corrected),
            "{reader} corrected nothing"
        );
    }
    assert_eq!(run(&extended), expected, "after the rewrites:\n{extended}");
    let (plain, _) = correct(&extended, &["Lint/ExtendedSyntax"]);
    assert_eq!(run(&plain), expected, "after lowering:\n{plain}");
    let (_, left) = correct(&plain, &["Lint/ExtendedSyntax"]);
    let left: Vec<(u32, u32, &str)> = left
        .iter()
        .map(|offense| (offense.line, offense.col, offense.message.as_str()))
        .collect();
    assert!(left.is_empty(), "extensions left: {left:?}\n{plain}");
}

#[test]
#[ignore = "runs the LuaJIT at /home/luna/code/nl/LuaJIT"]
fn lowering_keeps_what_the_extended_corpus_prints() {
    let corpus = include_str!("extended/extended.lua");
    let expected = run(corpus);
    let (plain, _) = correct(corpus, &["Lint/ExtendedSyntax"]);
    assert_ne!(plain, corpus);
    assert_eq!(run(&plain), expected, "after lowering:\n{plain}");
    let (_, left) = correct(&plain, &["Lint/ExtendedSyntax"]);
    assert!(
        left.iter().all(|offense| !offense.corrected),
        "lowering is not idempotent:\n{plain}"
    );
    for subject in ["`?.`", "`??`", "`+=`", "The ternary"] {
        assert!(
            left.iter()
                .any(|offense| offense.message.starts_with(subject)),
            "no {subject} without a fix is left:\n{plain}"
        );
    }
}
