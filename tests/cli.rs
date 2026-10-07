use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REFERENCE_READERS: &str = "Layout/LineLength,Layout/TrailingWhitespace";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn proofreader(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proofreader"))
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .output()
        .expect("the binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn copy_fixture(name: &str, into: &Path) -> PathBuf {
    let target = into.join(name);
    fs::copy(fixtures().join(name), &target).expect("copy fixture");
    target
}

#[test]
fn clean_file_passes_with_all_readers() {
    let output = proofreader(&fixtures(), &["clean.lua"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(
        stdout(&output),
        "Inspecting 1 file\n.\n\n1 file inspected, no offenses detected\n"
    );
}

#[test]
fn offenses_are_reported_clang_style() {
    let output = proofreader(&fixtures(), &["offenses.lua", "--only", REFERENCE_READERS]);
    assert_eq!(output.status.code(), Some(1));
    let long = format!("local message = '{}'", "x".repeat(118));
    let expected = format!(
        "Inspecting 1 file\nC\n\nOffenses:\n\n\
         offenses.lua:1:12: C: [Correctable] Layout/TrailingWhitespace: Trailing whitespace detected.\n\
         local a = 1  \n           ^^\n\
         offenses.lua:2:12: C: [Correctable] Layout/TrailingWhitespace: Trailing whitespace detected.\n\
         local b = 2\t\n           ^\n\
         offenses.lua:3:121: C: Layout/LineLength: Line is too long. [136/120]\n\
         {long}\n{}^^^^^^^^^^^^^^^^\n\
         offenses.lua:6:13: C: [Correctable] Layout/TrailingWhitespace: Trailing whitespace detected.\n\
         return a + b   \n            ^^^\n\n\
         1 file inspected, 4 offenses detected, 3 offenses autocorrectable\n",
        " ".repeat(120)
    );
    assert_eq!(stdout(&output), expected);
}

#[test]
fn quiet_format_omits_progress() {
    let output = proofreader(
        &fixtures(),
        &["offenses.lua", "--only", "Layout/LineLength", "-f", "quiet"],
    );
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.starts_with("Offenses:\n\noffenses.lua:3:121: C: Layout/LineLength"));
    assert!(text.ends_with("\n\n1 file inspected, 1 offense detected\n"));
    let clean = proofreader(&fixtures(), &["clean.lua", "-f", "quiet"]);
    assert_eq!(clean.status.code(), Some(0));
    assert_eq!(stdout(&clean), "");
}

#[test]
fn offenses_format_counts_per_reader() {
    let output = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "--only",
            REFERENCE_READERS,
            "--format",
            "offenses",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "3  Layout/TrailingWhitespace [Correctable]\n1  Layout/LineLength\n--\n4  Total in 1 file\n"
    );
}

#[test]
fn files_format_lists_offending_files() {
    let output = proofreader(&fixtures(), &["--only", REFERENCE_READERS, "-f", "files"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "nested/deep/inner.lua\nnested/long.lua\noffenses.lua\n"
    );
}

#[test]
fn json_format() {
    let output = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "clean.lua",
            "--only",
            REFERENCE_READERS,
            "-f",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let json = stdout(&output);
    assert!(json.starts_with("{\"metadata\":{\"proofreader_version\":\""));
    assert!(json.contains("{\"path\":\"clean.lua\",\"offenses\":[]}"));
    assert!(json.contains(
        "{\"severity\":\"convention\",\"message\":\"Trailing whitespace detected.\",\
         \"reader_name\":\"Layout/TrailingWhitespace\",\"corrected\":false,\"correctable\":true,\
         \"location\":{\"start_line\":1,\"start_column\":12,\"last_line\":1,\"last_column\":13,\
         \"length\":2,\"line\":1,\"column\":12}}"
    ));
    assert!(json.contains(
        "\"summary\":{\"offense_count\":4,\"target_file_count\":2,\"inspected_file_count\":2,\
         \"corrected_count\":0,\"correctable_count\":3}"
    ));
}

#[test]
fn nested_config_inherits_and_overrides() {
    let output = proofreader(
        &fixtures(),
        &["nested", "--only", REFERENCE_READERS, "-f", "quiet"],
    );
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(
        text.contains("nested/long.lua:2:41: C: Layout/LineLength: Line is too long. [50/40]\n")
    );
    assert!(
        text.contains("nested/deep/inner.lua:1:24: C: [Correctable] Layout/TrailingWhitespace")
    );
    assert!(
        !text.contains("long.lua:3:"),
        "comments are ignored through inherit_from"
    );
    assert!(!text.contains("legacy.lua"), "reader-level Exclude applies");
    assert!(!text.contains("vendor"), "AllReaders Exclude applies");
    assert!(text.ends_with("3 files inspected, 2 offenses detected, 1 offense autocorrectable\n"));
}

#[test]
fn lists_target_files() {
    let output = proofreader(&fixtures(), &["-L"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout(&output),
        "clean.lua\nnested/deep/inner.lua\nnested/legacy.lua\nnested/long.lua\noffenses.lua\n"
    );
    let explicit = proofreader(
        &fixtures(),
        &["-L", "nested/vendor/third_party.lua", "nested/notes.txt"],
    );
    assert_eq!(
        stdout(&explicit),
        "nested/notes.txt\nnested/vendor/third_party.lua\n"
    );
}

#[test]
fn explicitly_given_files_are_always_inspected() {
    let output = proofreader(
        &fixtures(),
        &[
            "nested/vendor/third_party.lua",
            "--only",
            "Layout/TrailingWhitespace",
            "-f",
            "files",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "nested/vendor/third_party.lua\n");
}

#[test]
fn fix_writes_corrections_and_is_idempotent() {
    let dir = scratch("fix");
    let file = copy_fixture("offenses.lua", &dir);
    let output = proofreader(&dir, &["--fix", "--only", REFERENCE_READERS]);
    assert_eq!(output.status.code(), Some(1), "the long line remains");
    let text = stdout(&output);
    assert!(text.contains("offenses.lua:1:12: C: [Corrected] Layout/TrailingWhitespace"));
    assert!(text.contains("offenses.lua:3:121: C: Layout/LineLength"));
    assert!(text.ends_with("1 file inspected, 4 offenses detected, 3 offenses corrected\n"));
    let fixed = fs::read_to_string(&file).expect("fixed file");
    assert!(fixed.starts_with("local a = 1\nlocal b = 2\nlocal message"));
    assert!(fixed.ends_with("\n\nreturn a + b\n"));
    let again = proofreader(&dir, &["-a", "--only", REFERENCE_READERS]);
    assert!(stdout(&again).ends_with("1 file inspected, 1 offense detected\n"));
    assert_eq!(fs::read_to_string(&file).expect("file"), fixed);
    let passing = proofreader(&dir, &["--fix", "--only", "Layout/TrailingWhitespace"]);
    assert_eq!(passing.status.code(), Some(0));
}

#[test]
fn fix_with_diff_prints_without_writing() {
    let dir = scratch("diff");
    let file = copy_fixture("offenses.lua", &dir);
    let original = fs::read_to_string(&file).expect("file");
    let output = proofreader(
        &dir,
        &["--fix", "--diff", "--only", "Layout/TrailingWhitespace"],
    );
    let text = stdout(&output);
    assert!(text.contains("--- a/offenses.lua\n+++ b/offenses.lua\n@@ -1,6 +1,6 @@\n-local a = 1  \n-local b = 2\t\n+local a = 1\n+local b = 2\n"));
    assert!(text.contains("-return a + b   \n+return a + b\n"));
    assert_eq!(fs::read_to_string(&file).expect("file"), original);
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn only_and_except_select_readers() {
    let except = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "--only",
            "Layout",
            "--except",
            REFERENCE_READERS,
        ],
    );
    assert_eq!(except.status.code(), Some(0));
    assert!(stdout(&except).ends_with("1 file inspected, no offenses detected\n"));
    let short = proofreader(
        &fixtures(),
        &["offenses.lua", "--only", "linelength", "-f", "files"],
    );
    assert_eq!(short.status.code(), Some(1));
}

#[test]
fn inline_directives_disable_readers() {
    let dir = scratch("directives");
    let source = "local a = 1  \n\
                  local b = 2   -- proofreader-disable-line Layout/TrailingWhitespace\n\
                  -- proofreader-disable-next-line\n\
                  local c = 3  \n\
                  // proofreader-disable Layout\n\
                  local d = 4  \n\
                  // proofreader-enable\n\
                  local e = 5  \n";
    let file = dir.join("directives.lua");
    fs::write(&file, source).expect("write file");
    let args = ["--only", "Layout/TrailingWhitespace", "-f", "quiet"];
    let output = proofreader(&dir, &args);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("directives.lua:1:12:"));
    assert!(text.contains("directives.lua:8:12:"));
    assert!(text.ends_with("1 file inspected, 2 offenses detected, 2 offenses autocorrectable\n"));
    let fixed = proofreader(&dir, &[&args[..], &["--fix"]].concat());
    assert_eq!(fixed.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(&file).expect("file"),
        source.replace("1  \n", "1\n").replace("5  \n", "5\n")
    );
}

#[test]
fn fail_level_controls_the_exit_code() {
    let output = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "--only",
            REFERENCE_READERS,
            "--fail-level",
            "warning",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let output = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "--only",
            REFERENCE_READERS,
            "--fail-level",
            "convention",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn config_option_forces_one_file() {
    let output = proofreader(
        &fixtures(),
        &[
            "offenses.lua",
            "--config",
            "nested/.proofreader.yml",
            "--only",
            "Layout/LineLength",
            "-f",
            "quiet",
        ],
    );
    let text = stdout(&output);
    assert!(text.contains("offenses.lua:3:41: C: Layout/LineLength: Line is too long. [136/40]"));
    assert!(
        !text.contains("offenses.lua:4:"),
        "IgnoreComments comes from the inherited file"
    );
}

#[test]
fn show_readers_lists_and_describes() {
    let list = proofreader(&fixtures(), &["--show-readers"]);
    assert_eq!(list.status.code(), Some(0));
    let text = stdout(&list);
    assert!(text.contains("Layout/LineLength"));
    assert!(text.contains("Layout/TrailingWhitespace"));
    assert!(text.contains("Checks for trailing whitespace."));
    let nested = proofreader(
        &fixtures().join("nested"),
        &["--show-readers", "LineLength"],
    );
    assert_eq!(
        stdout(&nested),
        "Layout/LineLength:\n  Description: Checks that lines are not longer than the configured maximum.\n  \
         Enabled: true\n  Severity: convention\n  AutoCorrect: true\n  Max: 40\n  AllowURI: true\n  IgnoreComments: true\n  IndentationWidth: 2\n"
    );
    let unknown = proofreader(&fixtures(), &["--show-readers", "Nope"]);
    assert_eq!(unknown.status.code(), Some(2));
}

#[test]
fn usage_and_io_errors_exit_with_two() {
    let missing = proofreader(&fixtures(), &["does-not-exist.lua"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).contains("does-not-exist.lua does not exist"));
    let unknown = proofreader(&fixtures(), &["--only", "Style/Nope"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unrecognized reader or department: Style/Nope"));
    let bad_flag = proofreader(&fixtures(), &["--frobnicate"]);
    assert_eq!(bad_flag.status.code(), Some(2));
    let diff_without_fix = proofreader(&fixtures(), &["--diff"]);
    assert_eq!(diff_without_fix.status.code(), Some(2));
}

#[test]
fn broken_configs_are_errors_and_unknown_readers_warn() {
    let dir = scratch("configs");
    fs::write(dir.join("a.lua"), "local a = 1\n").expect("write");
    fs::write(dir.join(".proofreader.yml"), "Foo/Bar:\n  Enabled: false\n").expect("write");
    let warned = proofreader(&dir, &["--only", REFERENCE_READERS]);
    assert_eq!(warned.status.code(), Some(0));
    assert!(stderr(&warned).contains("Warning: unrecognized reader Foo/Bar found in "));
    fs::write(
        dir.join(".proofreader.yml"),
        "Layout/LineLength:\n  Severity: loud\n",
    )
    .expect("write");
    let broken = proofreader(&dir, &[]);
    assert_eq!(broken.status.code(), Some(2));
    assert!(stderr(&broken).contains("unknown severity `loud`"));
}

#[test]
fn version_flag() {
    let output = proofreader(&fixtures(), &["-v"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout(&output),
        format!("proofreader {}\n", env!("CARGO_PKG_VERSION"))
    );
}
