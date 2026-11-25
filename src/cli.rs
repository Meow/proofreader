//! Command-line interface.

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgAction, Parser, ValueEnum};

use crate::config::{CONFIG_FILE_NAME, ConfigStore, absolute_path, default_configuration};
use crate::formatter::{Format, Progress, render};
use crate::offense::Severity;
use crate::reader::{
    did_you_mean, find_reader, reader_named, reader_names, registry, suggest_reader,
};
use crate::runner::{
    FileReport, MAX_PASSES, Options, READER_ERROR, ReaderFilter, inspect_bytes, inspect_files,
    resolve_targets, stdin_target,
};

/// Help shown after the options: exit codes, configuration discovery and inline directives.
const AFTER_HELP: &str = "\
Exit codes:
  0  No offense at or above --fail-level remains (corrected offenses do not count).
  1  At least one uncorrected offense at or above --fail-level.
  2  Usage error, unknown reader, missing path, broken configuration, unreadable or unwritable
     file, a reader that crashed, autocorrection that did not settle, or (with --strict) a
     configuration warning.

Configuration:
  Each file is checked with the nearest .proofreader.yml in its directory or an ancestor, or
  with --config. A nested .proofreader.yml replaces its parent's unless it says
  `inherit_from: ../.proofreader.yml`. --init writes a starting file, --show-config prints the
  one in effect for a file.

Inline directives, in any comment (`--`, `//`, `--[[ ]]`, `/* */`):
  -- proofreader-disable [READERS]             up to proofreader-enable or the end of the file
  -- proofreader-enable [READERS]
  -- proofreader-disable-line [READERS]        on the line of the comment
  -- proofreader-disable-next-line [READERS]   on the line after it
  READERS are names, departments or bare names separated by commas or spaces (all readers when
  left out); text after a standalone `--` explains the directive.";

/// A starting configuration written by `--init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Style {
    /// Every setting with its built-in default (the Flux style without the LuaJIT 3.0 readers).
    Default,
    /// The Flux framework, with the readers writing the LuaJIT 3.0 extensions.
    Flux,
    /// The Flux style with Clockwork naming.
    Catwork,
    /// Clockwork: tabs, PascalCase functions, `;` after statements.
    Clockwork,
    /// The Lua shipped with Garry's Mod.
    Gmod,
    /// Helix schemas and plugins.
    Helix,
    /// C++ in the Stroustrup style.
    Stroustrup,
}

impl Style {
    /// The text of the configuration file.
    fn configuration(self) -> String {
        let example = match self {
            Style::Default => return default_configuration(),
            Style::Flux => include_str!("../examples/flux/.proofreader.yml"),
            Style::Catwork => include_str!("../examples/catwork/.proofreader.yml"),
            Style::Clockwork => include_str!("../examples/clockwork/.proofreader.yml"),
            Style::Gmod => include_str!("../examples/gmod/.proofreader.yml"),
            Style::Helix => include_str!("../examples/helix/.proofreader.yml"),
            Style::Stroustrup => include_str!("../examples/stroustrup/.proofreader.yml"),
        };
        format!(
            "# Written by `proofreader --init --style {}`; paths in the comments below refer to the\n\
             # examples folder of the proofreader repository.\n#\n{example}",
            self.name()
        )
    }

    /// The name of the style on the command line.
    fn name(self) -> String {
        self.to_possible_value()
            .map_or_else(String::new, |value| value.get_name().to_owned())
    }
}

/// Command-line arguments.
#[derive(Debug, Parser)]
#[command(
    name = "proofreader",
    version,
    about = "A RuboCop-like linter and autocorrector for GLua",
    disable_version_flag = true,
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// Files or directories to inspect [default: .].
    pub paths: Vec<PathBuf>,

    /// Inspect the source read from standard input as if it were the file PATH; with --fix,
    /// print only the corrected source.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["paths", "list_target_files"])]
    pub stdin: Option<PathBuf>,

    /// Fix correctable offenses in place, leaving out unsafe corrections.
    #[arg(short = 'a', long = "fix", visible_alias = "autocorrect")]
    pub fix: bool,

    /// Like --fix, and also apply unsafe corrections, which may change what the code does (such
    /// as deleting unused functions).
    #[arg(short = 'A', long = "fix-unsafe")]
    pub fix_unsafe: bool,

    /// Print the corrections as a diff instead of writing files (without --fix or --fix-unsafe,
    /// those of --fix).
    #[arg(long)]
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

    /// Print which configuration applies to FILE (default: the current directory) and every
    /// setting it gives.
    #[arg(long, value_name = "FILE", num_args = 0..=1, default_missing_value = ".")]
    pub show_config: Option<PathBuf>,

    /// Write a starting .proofreader.yml to the current directory; never overwrites one.
    #[arg(long)]
    pub init: bool,

    /// With --init: the style to start from.
    #[arg(long, value_enum, requires = "init", default_value_t = Style::Default)]
    pub style: Style,

    /// Exit with status 1 only for offenses at or above this severity: info, refactor,
    /// convention, warning, error or fatal.
    #[arg(long, value_name = "SEVERITY", default_value = "info")]
    pub fail_level: Severity,

    /// Maximum number of autocorrect passes per file.
    #[arg(long, value_name = "N", default_value_t = MAX_PASSES)]
    pub max_passes: usize,

    /// Skip the files given explicitly that AllReaders.Exclude matches, as directories do (for
    /// editors, hooks and pre-commit, which pass every changed file).
    #[arg(long)]
    pub force_exclusion: bool,

    /// Exit with status 2 when the configuration produces warnings (unknown names or values).
    #[arg(long)]
    pub strict: bool,

    /// Disable colored output (NO_COLOR and an output that is not a terminal disable it too).
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

/// Prints the warnings collected by `store` to stderr; returns whether there were any.
fn print_warnings(store: &mut ConfigStore) -> bool {
    let warnings = store.take_warnings();
    for warning in &warnings {
        eprintln!("{warning}");
    }
    !warnings.is_empty()
}

/// Prints the reader list, or the effective configuration of readers matching `pattern`.
fn show_readers(pattern: &str, store: &mut ConfigStore, out: &mut impl Write) -> Result<bool> {
    let config = store.for_dir(&absolute_path(Path::new(".")))?;
    let warned = print_warnings(store);
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
        return Ok(warned);
    }
    let mut names = Vec::new();
    for part in pattern.split(',').map(str::trim) {
        let found = find_reader(part);
        if found.is_empty() {
            bail!(
                "unrecognized reader or department: {part}{}",
                did_you_mean(suggest_reader(part))
            );
        }
        names.extend(
            found
                .into_iter()
                .filter(|name| reader_named(name).is_some()),
        );
    }
    out.write_all(config.describe(&names).as_bytes())?;
    Ok(warned)
}

/// Prints which configuration applies to `file` and every setting it gives.
fn show_config(file: &Path, store: &mut ConfigStore, out: &mut impl Write) -> Result<bool> {
    let absolute = absolute_path(file);
    let config = if absolute.is_dir() {
        store.for_dir(&absolute)?
    } else {
        store.for_file(&absolute)?
    };
    let warned = print_warnings(store);
    match config.path() {
        Some(path) => writeln!(
            out,
            "# Configuration of {}: {}",
            file.display(),
            path.display()
        )?,
        None => writeln!(
            out,
            "# Configuration of {}: the built-in defaults",
            file.display()
        )?,
    }
    let inherited: Vec<String> = config
        .files()
        .iter()
        .filter(|path| Some(path.as_path()) != config.path())
        .map(|path| path.display().to_string())
        .collect();
    if !inherited.is_empty() {
        writeln!(out, "# Inherits from: {}", inherited.join(", "))?;
    }
    writeln!(out)?;
    out.write_all(config.describe_all_readers().as_bytes())?;
    writeln!(out)?;
    out.write_all(config.describe(&reader_names()).as_bytes())?;
    Ok(warned)
}

/// Writes the configuration of `style` to `.proofreader.yml` in the current directory, unless
/// one exists.
fn init(style: Style, out: &mut impl Write) -> Result<()> {
    let path = Path::new(CONFIG_FILE_NAME);
    if path.exists() {
        bail!("{CONFIG_FILE_NAME} already exists; edit it or remove it first");
    }
    fs::write(path, style.configuration())?;
    writeln!(out, "Wrote {CONFIG_FILE_NAME} ({} style)", style.name())?;
    Ok(())
}

/// The exit status for `reports`: 2 for I/O errors, readers that crashed and autocorrection
/// that did not settle, 1 for uncorrected offenses at or above `fail_level`, 0 otherwise.
fn exit_status(reports: &[FileReport], fail_level: Severity) -> u8 {
    let broken = reports.iter().any(|report| {
        report.error.is_some()
            || !report.unsettled.is_empty()
            || report
                .offenses
                .iter()
                .any(|offense| offense.reader == READER_ERROR)
    });
    if broken {
        return 2;
    }
    let failing = reports
        .iter()
        .flat_map(|report| &report.offenses)
        .any(|offense| !offense.corrected && offense.severity >= fail_level);
    u8::from(failing)
}

/// The warning for a file whose autocorrection did not settle, naming the readers whose fixes
/// kept coming back.
fn unsettled_warning(report: &FileReport) -> String {
    let readers = report.unsettled.join(", ");
    let what = if report.unsettled.len() == 1 {
        format!("{readers} keeps changing it")
    } else {
        format!("{readers} undo each other")
    };
    format!(
        "Warning: autocorrection did not settle in {}: {what}",
        report.path.display()
    )
}

/// Runs the command line and returns the process exit status.
pub fn run(cli: Cli) -> Result<u8> {
    let color = use_color(&cli);
    let stdout = io::stdout();
    if cli.init {
        init(cli.style, &mut stdout.lock())?;
        return Ok(0);
    }
    let mut store = ConfigStore::new(cli.config.as_deref())?;
    let strict = |warned: bool, status: u8| if cli.strict && warned { 2 } else { status };
    if let Some(pattern) = &cli.show_readers {
        let warned = show_readers(pattern, &mut store, &mut stdout.lock())?;
        return Ok(strict(warned, 0));
    }
    if let Some(file) = &cli.show_config {
        let warned = show_config(file, &mut store, &mut stdout.lock())?;
        return Ok(strict(warned, 0));
    }
    let fixing = cli.fix || cli.fix_unsafe;
    let options = Options {
        fix: fixing || cli.diff,
        unsafe_fixes: cli.fix_unsafe,
        diff: cli.diff,
        filter: ReaderFilter::new(&cli.only, &cli.except)?,
        max_passes: cli.max_passes,
    };
    let mut warned = print_warnings(&mut store);
    let reports = if let Some(path) = &cli.stdin {
        let target = stdin_target(path, &mut store)?;
        warned |= print_warnings(&mut store);
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        let excluded = cli.force_exclusion && target.config.excludes_file(&target.absolute);
        let (report, corrected) = if excluded {
            let report = FileReport {
                path: target.path.clone(),
                offenses: Vec::new(),
                diff: None,
                error: None,
                unsettled: Vec::new(),
            };
            (report, None)
        } else {
            inspect_bytes(&target, bytes.clone(), &options)
        };
        if fixing && !cli.diff {
            if let Some(error) = &report.error {
                eprintln!("Error: {error}");
            }
            if !report.unsettled.is_empty() {
                eprintln!("{}", unsettled_warning(&report));
            }
            let mut out = stdout.lock();
            match corrected {
                Some(text) => out.write_all(text.as_bytes())?,
                None => out.write_all(&bytes)?,
            }
            out.flush()?;
            let status = exit_status(std::slice::from_ref(&report), cli.fail_level);
            return Ok(strict(warned, status));
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
        let targets = resolve_targets(&paths, &mut store, cli.force_exclusion)?;
        warned |= print_warnings(&mut store);
        if cli.list_target_files {
            let mut out = stdout.lock();
            for target in &targets {
                writeln!(out, "{}", target.path.display())?;
            }
            return Ok(strict(warned, 0));
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
    for report in reports.iter().filter(|report| !report.unsettled.is_empty()) {
        eprintln!("{}", unsettled_warning(report));
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
    Ok(strict(warned, exit_status(&reports, cli.fail_level)))
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
    fn crashed_readers_and_unsettled_corrections_exit_with_two() {
        let source = crate::source::Source::new("x.lua", "x = 1\n");
        let config = crate::config::ReaderConfig::default();
        let mut ctx = crate::reader::Context::new(&source, &config, READER_ERROR, Severity::Error);
        ctx.add_offense(0..1, "An error occurred.");
        let report = FileReport {
            path: PathBuf::from("x.lua"),
            offenses: ctx.into_offenses(),
            diff: None,
            error: None,
            unsettled: Vec::new(),
        };
        assert_eq!(
            exit_status(std::slice::from_ref(&report), Severity::Info),
            2
        );
        let unsettled = FileReport {
            offenses: Vec::new(),
            unsettled: vec!["Layout/ExtraSpacing"],
            ..report.clone()
        };
        assert_eq!(
            exit_status(std::slice::from_ref(&unsettled), Severity::Info),
            2
        );
        assert_eq!(
            unsettled_warning(&unsettled),
            "Warning: autocorrection did not settle in x.lua: Layout/ExtraSpacing keeps changing it"
        );
        let clean = FileReport {
            unsettled: Vec::new(),
            ..unsettled
        };
        assert_eq!(exit_status(&[clean], Severity::Info), 0);
    }

    #[test]
    fn every_style_is_a_valid_configuration() {
        for style in Style::value_variants() {
            let text = style.configuration();
            let config = crate::config::Config::from_yaml_str(&text, Path::new("."))
                .unwrap_or_else(|error| panic!("{style:?}: {error}"));
            assert!(config.warnings().is_empty(), "{style:?}");
        }
        assert!(
            Style::Flux
                .configuration()
                .starts_with("# Written by `proofreader --init --style flux`")
        );
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
        assert!(Cli::try_parse_from(["proofreader", "--diff"]).is_ok());
        let unsafe_fix =
            Cli::try_parse_from(["proofreader", "-A", "--max-passes", "3"]).expect("valid");
        assert!(unsafe_fix.fix_unsafe && !unsafe_fix.fix);
        assert_eq!(unsafe_fix.max_passes, 3);
        assert_eq!(defaults.max_passes, MAX_PASSES);
        assert!(Cli::try_parse_from(["proofreader", "--style", "flux"]).is_err());
        let init =
            Cli::try_parse_from(["proofreader", "--init", "--style", "flux"]).expect("valid");
        assert_eq!(init.style, Style::Flux);
        assert!(Cli::try_parse_from(["proofreader", "-f", "github"]).is_ok());
        assert!(Cli::try_parse_from(["proofreader", "-f", "compact"]).is_ok());
        let show = Cli::try_parse_from(["proofreader", "--show-config"]).expect("valid");
        assert_eq!(show.show_config, Some(PathBuf::from(".")));
        let show = Cli::try_parse_from(["proofreader", "--show-readers"]).expect("valid");
        assert_eq!(show.show_readers.as_deref(), Some(""));
        let autocorrect = Cli::try_parse_from(["proofreader", "--autocorrect"]).expect("valid");
        assert!(autocorrect.fix);
    }
}
