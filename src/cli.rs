//! Command-line interface.

use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{ArgAction, Parser};

use crate::config::{ConfigStore, absolute_path};
use crate::formatter::{Format, Progress, render};
use crate::offense::Severity;
use crate::reader::{find_reader, reader_named, registry};
use crate::runner::{
    FileReport, Options, ReaderFilter, inspect_bytes, inspect_files, resolve_targets, stdin_target,
};

/// Command-line arguments.
#[derive(Debug, Parser)]
#[command(
    name = "proofreader",
    version,
    about = "A RuboCop-like linter and autocorrector for GLua",
    disable_version_flag = true
)]
pub struct Cli {
    /// Files or directories to inspect [default: .].
    pub paths: Vec<PathBuf>,

    /// Inspect the source read from standard input as if it were the file PATH; with --fix,
    /// print only the corrected source.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["paths", "list_target_files"])]
    pub stdin: Option<PathBuf>,

    /// Fix correctable offenses in place.
    #[arg(short = 'a', long = "fix", visible_alias = "autocorrect")]
    pub fix: bool,

    /// With --fix: print the changes as a diff instead of writing files.
    #[arg(long, requires = "fix")]
    pub diff: bool,

    /// Comma-separated reader names or departments to run exclusively.
    #[arg(long, value_delimiter = ',', value_name = "READERS")]
    pub only: Vec<String>,

    /// Comma-separated reader names or departments to skip.
    #[arg(long, value_delimiter = ',', value_name = "READERS")]
    pub except: Vec<String>,

    /// Output format.
    #[arg(short, long, value_enum, default_value_t = Format::Progress)]
    pub format: Format,

    /// Use this configuration file for every target.
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// List readers with their description; with PATTERN also their effective configuration.
    #[arg(long, value_name = "PATTERN", num_args = 0..=1, default_missing_value = "")]
    pub show_readers: Option<String>,

    /// Exit with status 1 only for offenses at or above this severity.
    #[arg(long, value_name = "SEVERITY", default_value = "info")]
    pub fail_level: Severity,

    /// Disable colored output.
    #[arg(long)]
    pub no_color: bool,

    /// List the files that would be inspected and exit.
    #[arg(short = 'L', long)]
    pub list_target_files: bool,

    /// Print version.
    #[arg(short = 'v', long, action = ArgAction::Version)]
    pub version: Option<bool>,
}

/// Whether to emit ANSI colors: not disabled by flag or `NO_COLOR`, and stdout is a terminal.
fn use_color(cli: &Cli) -> bool {
    !cli.no_color
        && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
        && io::stdout().is_terminal()
}

/// Prints the reader list, or the effective configuration of readers matching `pattern`.
fn show_readers(pattern: &str, store: &mut ConfigStore, out: &mut impl Write) -> Result<u8> {
    let config = store.for_dir(&absolute_path(Path::new(".")))?;
    for warning in store.take_warnings() {
        eprintln!("{warning}");
    }
    if pattern.is_empty() {
        let width = registry()
            .iter()
            .map(|reader| reader.name().len())
            .max()
            .unwrap_or(0);
        for reader in registry() {
            let state = if config.reader(reader.name()).enabled {
                ""
            } else {
                " (disabled)"
            };
            writeln!(
                out,
                "{:<width$}  {}{state}",
                reader.name(),
                reader.description()
            )?;
        }
        writeln!(out, "\n{} readers", registry().len())?;
        return Ok(0);
    }
    let names: Vec<&str> = pattern
        .split(',')
        .flat_map(|part| find_reader(part.trim()))
        .filter(|name| reader_named(name).is_some())
        .collect();
    if names.is_empty() {
        anyhow::bail!("unrecognized reader or department: {pattern}");
    }
    out.write_all(config.describe(&names).as_bytes())?;
    Ok(0)
}

/// The exit status for `reports`: 2 for I/O errors, 1 for uncorrected offenses at or above
/// `fail_level`, 0 otherwise.
fn exit_status(reports: &[FileReport], fail_level: Severity) -> u8 {
    if reports.iter().any(|report| report.error.is_some()) {
        return 2;
    }
    let failing = reports
        .iter()
        .flat_map(|report| &report.offenses)
        .any(|offense| !offense.corrected && offense.severity >= fail_level);
    u8::from(failing)
}

/// Runs the command line and returns the process exit status.
pub fn run(cli: Cli) -> Result<u8> {
    let color = use_color(&cli);
    let stdout = io::stdout();
    let mut store = ConfigStore::new(cli.config.as_deref())?;
    if let Some(pattern) = &cli.show_readers {
        return show_readers(pattern, &mut store, &mut stdout.lock());
    }
    let options = Options {
        fix: cli.fix,
        diff: cli.diff,
        filter: ReaderFilter::new(&cli.only, &cli.except)?,
        ..Options::default()
    };
    let reports = if let Some(path) = &cli.stdin {
        let target = stdin_target(path, &mut store)?;
        for warning in store.take_warnings() {
            eprintln!("{warning}");
        }
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        let (report, corrected) = inspect_bytes(&target, bytes.clone(), &options);
        if cli.fix && !cli.diff {
            if let Some(error) = &report.error {
                eprintln!("Error: {error}");
            }
            let mut out = stdout.lock();
            match corrected {
                Some(text) => out.write_all(text.as_bytes())?,
                None => out.write_all(&bytes)?,
            }
            out.flush()?;
            return Ok(exit_status(std::slice::from_ref(&report), cli.fail_level));
        }
        if cli.format == Format::Progress {
            let progress = Progress::start(io::stdout(), 1, color)?;
            progress.file_done(0, &report);
            progress.finish()?;
        }
        vec![report]
    } else {
        let paths = if cli.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            cli.paths.clone()
        };
        let targets = resolve_targets(&paths, &mut store)?;
        for warning in store.take_warnings() {
            eprintln!("{warning}");
        }
        if cli.list_target_files {
            let mut out = stdout.lock();
            for target in &targets {
                writeln!(out, "{}", target.path.display())?;
            }
            return Ok(0);
        }
        if cli.format == Format::Progress {
            let progress = Progress::start(io::stdout(), targets.len(), color)?;
            let reports = inspect_files(&targets, &options, |index, report| {
                progress.file_done(index, report)
            });
            progress.finish()?;
            reports
        } else {
            inspect_files(&targets, &options, |_, _| {})
        }
    };
    for error in reports.iter().filter_map(|report| report.error.as_deref()) {
        eprintln!("Error: {error}");
    }
    let mut out = stdout.lock();
    if !matches!(cli.format, Format::Json | Format::Sarif) {
        for diff in reports.iter().filter_map(|report| report.diff.as_deref()) {
            if cli.format == Format::Progress {
                writeln!(out)?;
            }
            out.write_all(diff.as_bytes())?;
        }
    }
    out.write_all(render(cli.format, &reports, color).as_bytes())?;
    out.flush()?;
    Ok(exit_status(&reports, cli.fail_level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_flags() {
        let cli = Cli::try_parse_from([
            "proofreader",
            "lib",
            "-a",
            "--diff",
            "--only",
            "Layout,Style/Not",
            "-f",
            "json",
            "--fail-level",
            "warning",
        ])
        .expect("valid arguments");
        assert_eq!(cli.paths, vec![PathBuf::from("lib")]);
        assert!(cli.fix && cli.diff);
        assert_eq!(cli.only, vec!["Layout", "Style/Not"]);
        assert_eq!(cli.format, Format::Json);
        assert_eq!(cli.fail_level, Severity::Warning);
        let defaults = Cli::try_parse_from(["proofreader"]).expect("valid");
        assert!(defaults.paths.is_empty());
        assert!(defaults.stdin.is_none());
        let stdin =
            Cli::try_parse_from(["proofreader", "--stdin", "a/b.lua", "-a"]).expect("valid");
        assert_eq!(stdin.stdin, Some(PathBuf::from("a/b.lua")));
        assert!(Cli::try_parse_from(["proofreader", "--stdin", "a.lua", "lib"]).is_err());
        assert!(Cli::try_parse_from(["proofreader", "-f", "sarif"]).is_ok());
        assert_eq!(defaults.fail_level, Severity::Info);
        assert!(Cli::try_parse_from(["proofreader", "--diff"]).is_err());
        let show = Cli::try_parse_from(["proofreader", "--show-readers"]).expect("valid");
        assert_eq!(show.show_readers.as_deref(), Some(""));
        let autocorrect = Cli::try_parse_from(["proofreader", "--autocorrect"]).expect("valid");
        assert!(autocorrect.fix);
    }
}
