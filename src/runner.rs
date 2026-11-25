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

use crate::config::{Config, ConfigError, ConfigStore, absolute_path};
use crate::corrector;
use crate::directive::Directives;
use crate::offense::{Offense, Severity};
use crate::reader::{Context, Reader, did_you_mean, find_reader, registry, suggest_reader};
use crate::readers::lint::ast;
use crate::source::Source;

/// Name used for offenses reporting a reader that panicked.
pub const READER_ERROR: &str = "Lint/ReaderError";

/// The reader whose `fatal` offenses (code that does not parse) stop the autocorrection of a
/// file, and which reports files nesting too deeply for the readers that parse them.
pub const SYNTAX: &str = "Lint/Syntax";

/// Default maximum number of autocorrect passes per file.
pub const MAX_PASSES: usize = 25;

/// An error that stops a run before any file is inspected.
#[derive(Debug, Error)]
pub enum RunError {
    /// A target path does not exist.
    #[error("{} does not exist", .0.display())]
    MissingPath(PathBuf),
    /// `--only` or `--except` named nothing known.
    #[error("unrecognized reader or department: {name}{}", did_you_mean(*suggestion))]
    UnknownReader {
        /// The name as given.
        name: String,
        /// The nearest known name.
        suggestion: Option<&'static str>,
    },
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
                    return Err(RunError::UnknownReader {
                        name: pattern.to_owned(),
                        suggestion: suggest_reader(pattern),
                    });
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
    /// With `fix`: also apply the fixes that may change what the code does.
    pub unsafe_fixes: bool,
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
            unsafe_fixes: false,
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
    /// With `--fix`, when autocorrection reached the pass limit with fixes still to apply: the
    /// readers whose fixes kept coming back, sorted. Empty otherwise.
    pub unsettled: Vec<&'static str>,
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
/// Directories are walked recursively, skipping hidden directories and symbolic links, and
/// keep the `.lua` and `.lumen` files selected by the applicable `AllReaders` `Include`/
/// `Exclude`. Files named explicitly are always inspected, unless `force_exclusion` is set and
/// `AllReaders.Exclude` matches them. A missing path is an error.
pub fn resolve_targets(
    paths: &[PathBuf],
    store: &mut ConfigStore,
    force_exclusion: bool,
) -> Result<Vec<Target>, RunError> {
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    let mut targets = Vec::new();
    for path in paths {
        let absolute = absolute_path(path);
        if absolute.is_file() {
            if found.insert(absolute.clone()) {
                let config = store.for_file(&absolute)?;
                if force_exclusion && config.excludes_file(&absolute) {
                    continue;
                }
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
                || entry
                    .path()
                    .extension()
                    .is_none_or(|ext| ext != "lua" && ext != "lumen")
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
    configuration: &Config,
    severity: Severity,
) -> Result<Vec<Offense>, String> {
    install_panic_filter();
    let config = configuration.reader(reader.name());
    let mut ctx =
        Context::new(source, config, reader.name(), severity).with_configuration(configuration);
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
    let (excerpt, _) = source.excerpt(1, 0);
    Offense {
        reader: READER_ERROR,
        severity: Severity::Error,
        message: format!("An error occurred while {reader} was inspecting this file: {message}"),
        range: 0..0,
        line: 1,
        col: 1,
        char_col: 1,
        last_line: 1,
        last_col: 1,
        last_char_col: 1,
        source_line: excerpt.to_owned(),
        excerpt_start: 0,
        fix: None,
        unsafe_fix: false,
        corrected: false,
    }
}

/// What [`inspect`] found in one source.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    /// The offenses corrected in any pass plus those remaining in the last one, sorted.
    pub offenses: Vec<Offense>,
    /// The corrected text, when it differs from the input.
    pub corrected: Option<String>,
    /// When autocorrection reached `max_passes` with fixes still to apply: the readers whose
    /// fixes were applied in the last pass or are still pending, sorted. Empty otherwise.
    pub unsettled: Vec<&'static str>,
}

/// [`inspect`], returning the offenses and the corrected text.
pub fn inspect_source(
    source: &Source,
    config: &Config,
    options: &Options,
) -> (Vec<Offense>, Option<String>) {
    let inspection = inspect(source, config, options);
    (inspection.offenses, inspection.corrected)
}

/// Sorts `offenses` by line, column, reader and message, corrected ones first.
fn sort_offenses(offenses: &mut [Offense]) {
    offenses.sort_by(|a, b| {
        (a.line, a.col, a.reader, &a.message, !a.corrected).cmp(&(
            b.line,
            b.col,
            b.reader,
            &b.message,
            !b.corrected,
        ))
    });
}

/// Whether `offense` says the file does not parse, which stops its autocorrection.
fn stops_corrections(offense: &Offense) -> bool {
    offense.reader == SYNTAX && offense.severity == Severity::Fatal
}

/// Reports an offense of `reader` at `severity` covering `range` of `source`, outside any
/// reader's run.
fn notice(
    source: &Source,
    config: &Config,
    reader: &'static str,
    severity: Severity,
    range: std::ops::Range<usize>,
    message: String,
) -> Offense {
    let mut ctx = Context::new(source, config.reader(reader), reader, severity);
    ctx.add_offense(range, message);
    ctx.into_offenses().remove(0)
}

/// The fixes of `offenses` that a pass applies under `options`, with their indexes.
fn applicable_fixes(
    offenses: &[Offense],
    options: &Options,
) -> Vec<(usize, Vec<crate::offense::Edit>)> {
    offenses
        .iter()
        .enumerate()
        .filter(|(_, offense)| options.unsafe_fixes || !offense.unsafe_fix)
        .filter_map(|(index, offense)| offense.fix.clone().map(|fix| (index, fix)))
        .collect()
}

/// Runs every enabled reader over `source`, dropping the offenses switched off by inline
/// [directives](crate::directive) before they are reported or corrected.
///
/// With `options.fix`, fixes of correctable offenses are applied (unsafe ones only with
/// `options.unsafe_fixes`) and the text is re-inspected until a pass applies nothing or
/// `max_passes` passes have applied fixes, after which the text is inspected once more. A file
/// with a `fatal` [`SYNTAX`] offense is not corrected. When readers skip the file because it
/// nests too deeply for them to parse, an `info` [`SYNTAX`] offense names them. The offenses are
/// those corrected in any pass plus those remaining in the last pass, de-duplicated by
/// [`Offense::key`] (corrected ones win) and sorted.
pub fn inspect(source: &Source, config: &Config, options: &Options) -> Inspection {
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
    let reports_depth = active.iter().any(|reader| reader.name() == SYNTAX);
    let passes = options.max_passes.max(1);
    let mut kept = Vec::new();
    let mut remaining = Vec::new();
    let mut current: Option<Source> = None;
    let mut last_applied: BTreeSet<&'static str> = BTreeSet::new();
    let mut unsettled = Vec::new();
    for pass in 0..=passes {
        let pass_source = current.as_ref().unwrap_or(source);
        let mut offenses = Vec::new();
        let mut skipped = Vec::new();
        let mut deepest: Option<usize> = None;
        active.retain(|reader| {
            ast::take_too_deep();
            let outcome = investigate(*reader, pass_source, config, config.severity_of(*reader));
            if let Some(offset) = ast::take_too_deep().filter(|_| reader.name() != SYNTAX) {
                skipped.push(reader.name());
                deepest = Some(deepest.map_or(offset, |earlier| earlier.min(offset)));
            }
            match outcome {
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
        if let Some(offset) = deepest.filter(|_| reports_depth) {
            offenses.push(notice(
                pass_source,
                config,
                SYNTAX,
                Severity::Info,
                offset..offset,
                format!(
                    "The code nests too deeply to be inspected; {} skipped the file.",
                    skipped.join(", ")
                ),
            ));
        }
        let directives = Directives::parse(pass_source);
        if !directives.is_empty() {
            offenses.retain(|offense| !directives.suppresses(offense));
        }
        if !options.fix || offenses.iter().any(stops_corrections) {
            remaining = offenses;
            break;
        }
        let (text, applied) =
            corrector::apply(&pass_source.text, applicable_fixes(&offenses, options));
        if pass == passes && !applied.is_empty() {
            last_applied.extend(applied.iter().map(|&index| offenses[index].reader));
            unsettled = last_applied.into_iter().collect();
        }
        if pass == passes || applied.is_empty() {
            remaining = offenses;
            break;
        }
        last_applied = applied
            .iter()
            .map(|&index| offenses[index].reader)
            .collect();
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
    sort_offenses(&mut kept);
    kept.dedup_by(|later, earlier| later.key() == earlier.key());
    let corrected = current
        .map(|fixed| fixed.text)
        .filter(|text| *text != source.text);
    Inspection {
        offenses: kept,
        corrected,
        unsettled,
    }
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
    let bytes = match fs::read(&target.absolute) {
        Ok(bytes) => bytes,
        Err(error) => {
            return FileReport {
                path: target.path.clone(),
                offenses: Vec::new(),
                diff: None,
                error: Some(format!("cannot read {}: {error}", target.path.display())),
                unsettled: Vec::new(),
            };
        }
    };
    let (mut report, corrected) = inspect_bytes(target, bytes, options);
    if let Some(corrected) = corrected
        && report.error.is_none()
        && !options.diff
        && let Err(error) = fs::write(&target.absolute, corrected)
    {
        report.error = Some(format!("cannot write {}: {error}", target.path.display()));
    }
    report
}

/// Inspects `bytes` as the contents of `target` without touching the file system.
///
/// Returns the report (with a diff under `options.diff`) and, with `options.fix`, the corrected
/// text when it differs from the input. Text that is not valid UTF-8 is inspected lossily
/// without corrections and gets a `fatal` [`SYNTAX`] offense at the first invalid byte; with
/// `options.fix`, offenses that could have been corrected make the report an error.
pub fn inspect_bytes(
    target: &Target,
    bytes: Vec<u8>,
    options: &Options,
) -> (FileReport, Option<String>) {
    let mut report = FileReport {
        path: target.path.clone(),
        offenses: Vec::new(),
        diff: None,
        error: None,
        unsettled: Vec::new(),
    };
    let (text, invalid) = match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(error) => {
            let valid = error.utf8_error().valid_up_to();
            (
                String::from_utf8_lossy(error.as_bytes()).into_owned(),
                Some(valid),
            )
        }
    };
    let source = Source::new(target.path.clone(), text);
    if let Some(valid) = invalid {
        let inspection = inspect(
            &source,
            &target.config,
            &Options {
                fix: false,
                diff: false,
                ..options.clone()
            },
        );
        report.offenses = inspection.offenses;
        let end = source.text[valid..]
            .chars()
            .next()
            .map_or(valid, |c| valid + c.len_utf8());
        report.offenses.push(notice(
            &source,
            &target.config,
            SYNTAX,
            Severity::Fatal,
            valid..end,
            format!("File is not valid UTF-8 (byte {}).", valid + 1),
        ));
        sort_offenses(&mut report.offenses);
        if options.fix && report.offenses.iter().any(Offense::correctable) {
            report.error = Some(format!(
                "{} is not valid UTF-8; corrections were not written",
                target.path.display()
            ));
        }
        return (report, None);
    }
    let inspection = inspect(&source, &target.config, options);
    report.offenses = inspection.offenses;
    report.unsettled = inspection.unsettled;
    let Some(corrected) = inspection.corrected else {
        return (report, None);
    };
    if options.diff {
        report.diff = Some(unified_diff(&target.path, &source.text, &corrected));
    }
    (report, Some(corrected))
}

/// The target for text read from standard input and inspected as if it were the file `path`,
/// which need not exist: its configuration is resolved for `path`.
pub fn stdin_target(path: &Path, store: &mut ConfigStore) -> Result<Target, RunError> {
    let absolute = absolute_path(path);
    let config = store.for_file(&absolute)?;
    Ok(Target {
        path: display_path(&absolute),
        absolute,
        config,
    })
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
            &Config::defaults(Path::new(".")),
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
        let typo = ReaderFilter::new(&[], &["Style/Nott".into()]).expect_err("unknown");
        assert_eq!(
            typo.to_string(),
            "unrecognized reader or department: Style/Nott; did you mean Style/Not?"
        );
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
    fn unsafe_fixes_need_their_own_option() {
        let config = Config::defaults(Path::new("."));
        let safe = Options {
            fix: true,
            filter: ReaderFilter::single("Lint/UnusedVariable"),
            ..Options::default()
        };
        let text = "local function helper() end\nlocal x = f()\n";
        let (offenses, corrected) = inspect_source(&Source::new("x.lua", text), &config, &safe);
        assert_eq!(
            corrected.as_deref(),
            Some("local function helper() end\nlocal _x = f()\n")
        );
        assert_eq!(
            offenses
                .iter()
                .map(|offense| (offense.line, offense.corrected))
                .collect::<Vec<_>>(),
            vec![(1, false), (2, true)]
        );
        let all = Options {
            unsafe_fixes: true,
            ..safe
        };
        let (_, corrected) = inspect_source(&Source::new("x.lua", text), &config, &all);
        assert_eq!(corrected.as_deref(), Some("local _x = f()\n"));
    }

    #[test]
    fn files_too_deep_to_parse_say_which_readers_skipped_them() {
        let config = Config::defaults(Path::new("."));
        let options = Options {
            filter: ReaderFilter::new(&["Lint/Syntax".into(), "Lint/SelfAssignment".into()], &[])
                .expect("known"),
            ..Options::default()
        };
        let deep = format!("y = y\nx = {}{}\n", "{".repeat(150), "}".repeat(150));
        let (offenses, _) = inspect_source(&Source::new("x.lua", deep), &config, &options);
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            (offenses[0].reader, offenses[0].severity, offenses[0].line),
            (SYNTAX, Severity::Info, 2)
        );
        assert_eq!(
            offenses[0].message,
            "The code nests too deeply to be inspected; Lint/SelfAssignment skipped the file."
        );
        let chain = format!("y = y\nx = {}\n", vec!["a"; 150].join(" .. "));
        let (offenses, _) = inspect_source(&Source::new("x.lua", chain), &config, &options);
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].reader, "Lint/SelfAssignment");
    }

    #[test]
    fn files_that_do_not_parse_are_not_corrected() {
        let config = Config::defaults(Path::new("."));
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let broken = "function f()\nif x then\n  y()\nend\n\nfunction g()\nend\n";
        let (offenses, corrected) =
            inspect_source(&Source::new("x.lua", broken), &config, &options);
        assert!(corrected.is_none());
        assert!(offenses.iter().any(stops_corrections));
        assert!(offenses.iter().all(|offense| !offense.corrected));
    }

    #[test]
    fn corrections_that_never_settle_are_reported() {
        let config = Config::from_yaml_str(
            "Layout/ExtraSpacing:\n  ForceEqualSignAlignment: true\n\
             Layout/SpaceAroundOperators:\n  AllowForAlignment: false\n",
            Path::new("."),
        )
        .expect("valid");
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let source = Source::new("x.lua", "local a = 1\nlocal bbb = 2\n");
        let inspection = inspect(&source, &config, &options);
        assert_eq!(
            inspection.unsettled,
            vec!["Layout/ExtraSpacing", "Layout/SpaceAroundOperators"]
        );
        assert!(inspection.offenses.iter().any(|offense| !offense.corrected));
        let settled = inspect(&source, &Config::defaults(Path::new(".")), &options);
        assert!(settled.unsettled.is_empty());
    }

    #[test]
    fn inline_disabled_offenses_are_neither_reported_nor_corrected() {
        let config = Config::defaults(Path::new("."));
        let options = Options {
            fix: true,
            filter: ReaderFilter::single("Layout/TrailingWhitespace"),
            ..Options::default()
        };
        let text = "a = 1  \n-- proofreader-disable-next-line Layout/TrailingWhitespace\nb = 2  \n";
        let (offenses, corrected) = inspect_source(&Source::new("x.lua", text), &config, &options);
        assert_eq!(
            corrected.as_deref(),
            Some("a = 1\n-- proofreader-disable-next-line Layout/TrailingWhitespace\nb = 2  \n")
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].corrected), (1, true));
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
    fn inspects_bytes_without_writing() {
        let target = Target {
            path: PathBuf::from("missing/x.lua"),
            absolute: absolute_path(Path::new("missing/x.lua")),
            config: Arc::new(Config::defaults(Path::new("."))),
        };
        let options = Options {
            fix: true,
            filter: ReaderFilter::single("Layout/TrailingWhitespace"),
            ..Options::default()
        };
        let (report, corrected) = inspect_bytes(&target, b"a = 1 \n".to_vec(), &options);
        assert_eq!(corrected.as_deref(), Some("a = 1\n"));
        assert!(report.error.is_none() && report.diff.is_none());
        assert!(report.offenses[0].corrected);
        let (report, corrected) = inspect_bytes(&target, b"a = '\xff' \n".to_vec(), &options);
        assert!(corrected.is_none());
        assert!(report.error.is_some());
        assert_eq!(
            report
                .offenses
                .iter()
                .map(|offense| (offense.reader, offense.col, offense.corrected))
                .collect::<Vec<_>>(),
            vec![(SYNTAX, 6, false), ("Layout/TrailingWhitespace", 10, false)]
        );
        assert_eq!(
            report.offenses[0].message,
            "File is not valid UTF-8 (byte 6)."
        );
        let check = Options {
            fix: false,
            ..options.clone()
        };
        let (report, _) = inspect_bytes(&target, b"a = '\xff'\n".to_vec(), &check);
        assert!(report.error.is_none());
        assert_eq!(report.offenses.len(), 1);
        let diff = Options {
            diff: true,
            ..options
        };
        let (report, _) = inspect_bytes(&target, b"a = 1 \n".to_vec(), &diff);
        assert!(
            report
                .diff
                .is_some_and(|diff| diff.contains("-a = 1 \n+a = 1\n"))
        );
    }

    #[test]
    fn explicit_files_honor_exclude_with_force_exclusion() {
        let dir =
            std::env::temp_dir().join(format!("proofreader-exclusion-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("vendor")).expect("dir");
        fs::write(
            dir.join(".proofreader.yml"),
            "AllReaders:\n  Exclude: ['vendor/**']\n",
        )
        .expect("config");
        fs::write(dir.join("vendor/x.lua"), "x = 1\n").expect("file");
        fs::write(dir.join("y.lua"), "y = 1\n").expect("file");
        let paths = [dir.join("vendor/x.lua"), dir.join("y.lua")];
        let mut store = ConfigStore::new(None).expect("store");
        let all = resolve_targets(&paths, &mut store, false).expect("targets");
        assert_eq!(all.len(), 2);
        let honored = resolve_targets(&paths, &mut store, true).expect("targets");
        assert_eq!(honored.len(), 1);
        assert!(honored[0].absolute.ends_with("y.lua"));
        assert!(
            resolve_targets(std::slice::from_ref(&dir), &mut store, false)
                .expect("targets")
                .iter()
                .all(|target| !target.absolute.starts_with(dir.join("vendor")))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn diffs_are_unified() {
        let diff = unified_diff(Path::new("a.lua"), "x \ny\n", "x\ny\n");
        assert!(diff.starts_with("--- a/a.lua\n+++ b/a.lua\n@@ -1,2 +1,2 @@\n-x \n+x\n y\n"));
    }
}
