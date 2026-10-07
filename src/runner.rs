//! Running readers over sources and files: target resolution, the autocorrect loop and
//! parallel inspection.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use rayon::prelude::*;
use similar::TextDiff;
use thiserror::Error;
use walkdir::WalkDir;

use crate::config::{Config, ConfigError, ConfigStore, ReaderConfig, absolute_path};
use crate::corrector;
use crate::offense::{Offense, Severity};
use crate::reader::{Context, Reader, find_reader, registry};
use crate::source::Source;

/// Name used for offenses reporting a reader that panicked.
pub const READER_ERROR: &str = "Lint/ReaderError";

/// Default maximum number of autocorrect passes per file.
pub const MAX_PASSES: usize = 10;

/// An error that stops a run before any file is inspected.
#[derive(Debug, Error)]
pub enum RunError {
    /// A target path does not exist.
    #[error("{} does not exist", .0.display())]
    MissingPath(PathBuf),
    /// `--only` or `--except` named nothing known.
    #[error("unrecognized reader or department: {0}")]
    UnknownReader(String),
    /// A configuration file is broken.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A directory could not be walked.
    #[error("cannot walk {}: {message}", path.display())]
    Walk {
        /// The directory being walked.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
}

/// Which readers run, from `--only` and `--except`.
#[derive(Debug, Clone, Default)]
pub struct ReaderFilter {
    only: Option<BTreeSet<&'static str>>,
    forced: BTreeSet<&'static str>,
    except: BTreeSet<&'static str>,
}

impl ReaderFilter {
    /// Resolves reader names, departments or short names; unknown entries are an error.
    pub fn new(only: &[String], except: &[String]) -> Result<Self, RunError> {
        let resolve = |patterns: &[String]| -> Result<BTreeSet<&'static str>, RunError> {
            let mut names = BTreeSet::new();
            for pattern in patterns
                .iter()
                .map(|pattern| pattern.trim())
                .filter(|pattern| !pattern.is_empty())
            {
                let found = find_reader(pattern);
                if found.is_empty() {
                    return Err(RunError::UnknownReader(pattern.to_owned()));
                }
                names.extend(found);
            }
            Ok(names)
        };
        let forced = only
            .iter()
            .flat_map(|pattern| find_reader(pattern.trim()))
            .filter(|name| {
                name.contains('/')
                    && only
                        .iter()
                        .any(|pattern| pattern.trim().eq_ignore_ascii_case(name))
            })
            .collect();
        Ok(ReaderFilter {
            only: if only.is_empty() {
                None
            } else {
                Some(resolve(only)?)
            },
            forced,
            except: resolve(except)?,
        })
    }

    /// Restricts the run to exactly the reader `name`, even if it is disabled.
    pub fn single(name: &'static str) -> Self {
        ReaderFilter {
            only: Some(BTreeSet::from([name])),
            forced: BTreeSet::from([name]),
            except: BTreeSet::new(),
        }
    }

    /// Whether the reader `name` runs, given whether the configuration enables it.
    ///
    /// Readers named explicitly in `--only` run even when disabled.
    pub fn allows(&self, name: &str, enabled: bool) -> bool {
        if self.except.contains(name) {
            return false;
        }
        match &self.only {
            Some(only) => only.contains(name) && (enabled || self.forced.contains(name)),
            None => enabled,
        }
    }
}

/// Options for a run.
#[derive(Debug, Clone)]
pub struct Options {
    /// Apply fixes.
    pub fix: bool,
    /// With `fix`: produce diffs instead of writing files.
    pub diff: bool,
    /// Which readers run.
    pub filter: ReaderFilter,
    /// Maximum number of autocorrect passes per file.
    pub max_passes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fix: false,
            diff: false,
            filter: ReaderFilter::default(),
            max_passes: MAX_PASSES,
        }
    }
}

/// A file to inspect with the configuration that applies to it.
#[derive(Debug, Clone)]
pub struct Target {
    /// Path shown in reports: relative to the working directory when below it.
    pub path: PathBuf,
    /// Normalised absolute path, used for reading, writing and glob matching.
    pub absolute: PathBuf,
    /// The applicable configuration.
    pub config: Arc<Config>,
}

/// The outcome of inspecting one file.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// Display path.
    pub path: PathBuf,
    /// Offenses sorted by line, column and reader.
    pub offenses: Vec<Offense>,
    /// Unified diff of the corrections, with `--fix --diff`.
    pub diff: Option<String>,
    /// I/O error that prevented inspecting or writing the file.
    pub error: Option<String>,
}

/// Path shown for `absolute`: relative to the working directory when below it.
fn display_path(absolute: &Path) -> PathBuf {
    let cwd = absolute_path(Path::new("."));
    match absolute.strip_prefix(&cwd) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_path_buf(),
        _ => absolute.to_path_buf(),
    }
}

/// Whether a directory entry below the root is hidden (its name starts with a dot).
fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.')
}

/// Expands `paths` into the sorted, de-duplicated list of files to inspect.
///
/// Directories are walked recursively, skipping hidden directories, and keep the `.lua` files
/// selected by the applicable `AllReaders` `Include`/`Exclude`. Files named explicitly are
/// always inspected. A missing path is an error.
pub fn resolve_targets(
    paths: &[PathBuf],
    store: &mut ConfigStore,
) -> Result<Vec<Target>, RunError> {
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    let mut targets = Vec::new();
    for path in paths {
        let absolute = absolute_path(path);
        if absolute.is_file() {
            if found.insert(absolute.clone()) {
                let config = store.for_file(&absolute)?;
                targets.push(Target {
                    path: display_path(&absolute),
                    absolute,
                    config,
                });
            }
            continue;
        }
        if !absolute.is_dir() {
            return Err(RunError::MissingPath(path.clone()));
        }
        let walker = WalkDir::new(&absolute)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| !(entry.file_type().is_dir() && is_hidden(entry)));
        for entry in walker {
            let entry = entry.map_err(|error| RunError::Walk {
                path: absolute.clone(),
                message: error.to_string(),
            })?;
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "lua")
            {
                continue;
            }
            let file = entry.path().to_path_buf();
            if found.contains(&file) {
                continue;
            }
            let config = store.for_file(&file)?;
            if config.includes_file(&file) {
                found.insert(file.clone());
                targets.push(Target {
                    path: display_path(&file),
                    absolute: file,
                    config,
                });
            }
        }
    }
    targets.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(targets)
}

thread_local! {
    static SILENT_PANICS: Cell<bool> = const { Cell::new(false) };
}

/// Installs (once) a panic hook that stays quiet while a reader runs under `catch_unwind`.
fn install_panic_filter() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !SILENT_PANICS.with(Cell::get) {
                previous(info);
            }
        }));
    });
}

/// Runs one reader over `source`, turning a panic into an error message.
pub fn investigate(
    reader: &dyn Reader,
    source: &Source,
    config: &ReaderConfig,
    severity: Severity,
) -> Result<Vec<Offense>, String> {
    install_panic_filter();
    let mut ctx = Context::new(source, config, reader.name(), severity);
    SILENT_PANICS.with(|flag| flag.set(true));
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| reader.investigate(&mut ctx)));
    SILENT_PANICS.with(|flag| flag.set(false));
    match outcome {
        Ok(()) => {
            let mut offenses = ctx.into_offenses();
            if !config.autocorrect {
                for offense in &mut offenses {
                    offense.fix = None;
                }
            }
            Ok(offenses)
        }
        Err(payload) => Err(payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned())),
    }
}

/// Builds the offense reported when `reader` panics.
fn reader_error(reader: &str, message: &str, source: &Source) -> Offense {
    Offense {
        reader: READER_ERROR,
        severity: Severity::Error,
        message: format!("An error occurred while {reader} was inspecting this file: {message}"),
        range: 0..0,
        line: 1,
        col: 1,
        last_line: 1,
        last_col: 1,
        source_line: source.line(1).to_owned(),
        fix: None,
        corrected: false,
    }
}

/// Runs every enabled reader over `source`.
///
/// With `options.fix`, fixes of correctable offenses are applied and the text is re-inspected
/// until a pass applies nothing or `max_passes` is reached. The result holds the offenses
/// corrected in any pass plus those remaining in the last pass, de-duplicated by
/// [`Offense::key`] (corrected ones win) and sorted; the second element is the corrected text
/// when it differs from the input.
pub fn inspect_source(
    source: &Source,
    config: &Config,
    options: &Options,
) -> (Vec<Offense>, Option<String>) {
    let absolute = absolute_path(&source.path);
    let mut active: Vec<&dyn Reader> = registry()
        .iter()
        .map(AsRef::as_ref)
        .filter(|reader| {
            let reader_config = config.reader(reader.name());
            options.filter.allows(reader.name(), reader_config.enabled)
                && reader_config.applies_to(&absolute)
        })
        .collect();
    let mut kept = Vec::new();
    let mut remaining = Vec::new();
    let mut current: Option<Source> = None;
    for _ in 0..options.max_passes.max(1) {
        let pass_source = current.as_ref().unwrap_or(source);
        let mut offenses = Vec::new();
        active.retain(|reader| {
            let reader_config = config.reader(reader.name());
            match investigate(
                *reader,
                pass_source,
                reader_config,
                config.severity_of(*reader),
            ) {
                Ok(found) => {
                    offenses.extend(found);
                    true
                }
                Err(message) => {
                    kept.push(reader_error(reader.name(), &message, pass_source));
                    false
                }
            }
        });
        if !options.fix {
            remaining = offenses;
            break;
        }
        let fixes = offenses
            .iter()
            .enumerate()
            .filter_map(|(index, offense)| offense.fix.clone().map(|fix| (index, fix)))
            .collect();
        let (text, applied) = corrector::apply(&pass_source.text, fixes);
        if applied.is_empty() {
            remaining = offenses;
            break;
        }
        for index in applied {
            offenses[index].corrected = true;
        }
        let (corrected, uncorrected): (Vec<Offense>, Vec<Offense>) =
            offenses.into_iter().partition(|offense| offense.corrected);
        kept.extend(corrected);
        remaining = uncorrected;
        current = Some(Source::new(source.path.clone(), text));
    }
    kept.extend(remaining);
    kept.sort_by(|a, b| {
        (a.line, a.col, a.reader, &a.message, !a.corrected).cmp(&(
            b.line,
            b.col,
            b.reader,
            &b.message,
            !b.corrected,
        ))
    });
    kept.dedup_by(|later, earlier| later.key() == earlier.key());
    let corrected = current
        .map(|fixed| fixed.text)
        .filter(|text| *text != source.text);
    (kept, corrected)
}

/// Renders a unified diff between the original and corrected text of `path`.
pub fn unified_diff(path: &Path, before: &str, after: &str) -> String {
    let shown = path.display().to_string();
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{shown}"), &format!("b/{shown}"))
        .to_string()
}

/// Reads, inspects and (with `fix`) writes back or diffs one file.
pub fn inspect_file(target: &Target, options: &Options) -> FileReport {
    let mut report = FileReport {
        path: target.path.clone(),
        offenses: Vec::new(),
        diff: None,
        error: None,
    };
    let bytes = match fs::read(&target.absolute) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.error = Some(format!("cannot read {}: {error}", target.path.display()));
            return report;
        }
    };
    let (text, lossless) = match String::from_utf8(bytes) {
        Ok(text) => (text, true),
        Err(error) => (
            String::from_utf8_lossy(error.as_bytes()).into_owned(),
            false,
        ),
    };
    let source = Source::new(target.path.clone(), text);
    let (offenses, corrected) = inspect_source(&source, &target.config, options);
    report.offenses = offenses;
    if let Some(corrected) = corrected {
        if !lossless {
            report.error = Some(format!(
                "{} is not valid UTF-8; corrections were not written",
                target.path.display()
            ));
        } else if options.diff {
            report.diff = Some(unified_diff(&target.path, &source.text, &corrected));
        } else if let Err(error) = fs::write(&target.absolute, corrected) {
            report.error = Some(format!("cannot write {}: {error}", target.path.display()));
        }
    }
    report
}

/// Inspects `targets` in parallel, calling `on_done(index, report)` as each file finishes.
/// Reports are returned in target order.
pub fn inspect_files(
    targets: &[Target],
    options: &Options,
    on_done: impl Fn(usize, &FileReport) + Sync,
) -> Vec<FileReport> {
    targets
        .par_iter()
        .enumerate()
        .map(|(index, target)| {
            let report = inspect_file(target, options);
            on_done(index, &report);
            report
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Reader;

    struct Panicky;

    impl Reader for Panicky {
        fn name(&self) -> &'static str {
            "Lint/Panicky"
        }

        fn description(&self) -> &'static str {
            "Always panics."
        }

        fn investigate(&self, _ctx: &mut Context) {
            panic!("boom");
        }
    }

    #[test]
    fn panicking_readers_are_caught() {
        let source = Source::new("x.lua", "local a\n");
        let result = investigate(
            &Panicky,
            &source,
            &ReaderConfig::default(),
            Severity::Warning,
        );
        assert_eq!(result, Err("boom".to_owned()));
        let offense = reader_error("Lint/Panicky", "boom", &source);
        assert_eq!(offense.severity, Severity::Error);
        assert_eq!((offense.line, offense.col), (1, 1));
    }

    #[test]
    fn filter_semantics() {
        let all = ReaderFilter::default();
        assert!(all.allows("Layout/LineLength", true));
        assert!(!all.allows("Layout/LineLength", false));
        let only = ReaderFilter::new(&["Layout/LineLength".into()], &[]).expect("known");
        assert!(only.allows("Layout/LineLength", false));
        assert!(!only.allows("Layout/TrailingWhitespace", true));
        let department =
            ReaderFilter::new(&["layout".into()], &["TrailingWhitespace".into()]).expect("known");
        assert!(department.allows("Layout/LineLength", true));
        assert!(!department.allows("Layout/LineLength", false));
        assert!(!department.allows("Layout/TrailingWhitespace", true));
        assert!(ReaderFilter::new(&["Nope".into()], &[]).is_err());
    }

    #[test]
    fn fix_loop_corrects_and_reports() {
        let config = Config::defaults(Path::new("."));
        let options = Options {
            fix: true,
            filter: ReaderFilter::single("Layout/TrailingWhitespace"),
            ..Options::default()
        };
        let source = Source::new("x.lua", "local a = 1  \nlocal b = 2\t\n");
        let (offenses, corrected) = inspect_source(&source, &config, &options);
        assert_eq!(corrected.as_deref(), Some("local a = 1\nlocal b = 2\n"));
        assert_eq!(offenses.len(), 2);
        assert!(offenses.iter().all(|offense| offense.corrected));
        let (again, unchanged) =
            inspect_source(&Source::new("x.lua", "local a = 1\n"), &config, &options);
        assert!(again.is_empty());
        assert!(unchanged.is_none());
    }

    #[test]
    fn autocorrect_can_be_disabled() {
        let config = Config::from_yaml_str(
            "Layout/TrailingWhitespace:\n  AutoCorrect: false\n",
            Path::new("."),
        )
        .expect("valid");
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let (offenses, corrected) =
            inspect_source(&Source::new("x.lua", "a = 1 \n"), &config, &options);
        assert!(corrected.is_none());
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }

    #[test]
    fn disabled_and_excluded_readers_do_not_run() {
        let config = Config::from_yaml_str(
            "Layout/TrailingWhitespace:\n  Enabled: false\nLayout/LineLength:\n  Exclude: ['x.lua']\n",
            Path::new("."),
        )
        .expect("valid");
        let long = format!("a = 1 \nb = '{}'\n", "x".repeat(200));
        let (offenses, _) = inspect_source(
            &Source::new("x.lua", long.clone()),
            &config,
            &Options::default(),
        );
        assert!(offenses.is_empty());
        let (offenses, _) =
            inspect_source(&Source::new("y.lua", long), &config, &Options::default());
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].reader, "Layout/LineLength");
    }

    #[test]
    fn diffs_are_unified() {
        let diff = unified_diff(Path::new("a.lua"), "x \ny\n", "x\ny\n");
        assert!(diff.starts_with("--- a/a.lua\n+++ b/a.lua\n@@ -1,2 +1,2 @@\n-x \n+x\n y\n"));
    }
}
