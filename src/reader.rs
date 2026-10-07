//! The [`Reader`] trait implemented by every check, the [`Context`] it reports into, and the
//! registry that collects readers through `inventory`.

use std::ops::Range;
use std::sync::OnceLock;

use yaml_rust2::Yaml;

use crate::config::ReaderConfig;
use crate::offense::{Edit, Offense, Severity};
use crate::source::Source;

/// A single check, named `Department/Name` (for example `Layout/TrailingWhitespace`).
///
/// Readers are stateless; [`Reader::investigate`] is called once per file and autocorrect pass.
pub trait Reader: Sync + Send {
    /// The full name, `Department/Name`.
    fn name(&self) -> &'static str;

    /// A one-sentence description shown by `--show-readers`.
    fn description(&self) -> &'static str;

    /// Severity used unless configured otherwise: `Warning` for `Lint/*`, `Convention` otherwise.
    fn default_severity(&self) -> Severity {
        if department(self.name()) == "Lint" {
            Severity::Warning
        } else {
            Severity::Convention
        }
    }

    /// Reader-specific options and their defaults, in display order.
    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        Vec::new()
    }

    /// Inspects `ctx.source` and reports offenses through `ctx`.
    fn investigate(&self, ctx: &mut Context);
}

/// What a reader sees while investigating one file, and where it reports offenses.
pub struct Context<'a> {
    /// The file being inspected.
    pub source: &'a Source,
    /// The effective configuration of the running reader for this file.
    pub config: &'a ReaderConfig,
    offenses: Vec<Offense>,
    reader: &'static str,
    severity: Severity,
}

impl<'a> Context<'a> {
    /// Creates a context for `reader` reporting offenses at `severity`.
    pub fn new(
        source: &'a Source,
        config: &'a ReaderConfig,
        reader: &'static str,
        severity: Severity,
    ) -> Self {
        Context {
            source,
            config,
            offenses: Vec::new(),
            reader,
            severity,
        }
    }

    /// Name of the running reader.
    pub fn reader_name(&self) -> &'static str {
        self.reader
    }

    /// Severity offenses are reported with.
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// Reports an offense covering `range`; chain [`Offense::with_fix`] to make it correctable.
    pub fn add_offense(&mut self, range: Range<usize>, message: impl Into<String>) -> &mut Offense {
        let (line, col) = self.source.position(range.start);
        let last = range.end.saturating_sub(1).max(range.start);
        let (last_line, last_col) = self.source.position(last);
        self.offenses.push(Offense {
            reader: self.reader,
            severity: self.severity,
            message: message.into(),
            range,
            line,
            col,
            last_line,
            last_col,
            source_line: self.source.line(line).to_owned(),
            fix: None,
            corrected: false,
        });
        let index = self.offenses.len() - 1;
        &mut self.offenses[index]
    }

    /// Reports an offense covering `range` that `fix` corrects.
    pub fn add_offense_with_fix(
        &mut self,
        range: Range<usize>,
        message: impl Into<String>,
        fix: Vec<Edit>,
    ) -> &mut Offense {
        self.add_offense(range, message).with_fix(fix)
    }

    /// Offenses reported so far.
    pub fn offenses(&self) -> &[Offense] {
        &self.offenses
    }

    /// Consumes the context, returning the reported offenses.
    pub fn into_offenses(self) -> Vec<Offense> {
        self.offenses
    }

    /// Boolean option `key` of the running reader.
    pub fn option_bool(&self, key: &str, default: bool) -> bool {
        self.config.get_bool(key, default)
    }

    /// Non-negative integer option `key` of the running reader.
    pub fn option_usize(&self, key: &str, default: usize) -> usize {
        self.config.get_usize(key, default)
    }

    /// String option `key` of the running reader.
    pub fn option_str(&self, key: &str, default: &'a str) -> &'a str {
        self.config.get_str(key, default)
    }

    /// String list option `key` of the running reader.
    pub fn option_str_list(&self, key: &str, default: &[&str]) -> Vec<String> {
        self.config.get_str_list(key, default)
    }
}

/// Registers a reader; submit one per reader with `inventory::submit!`.
pub struct Registration(pub fn() -> Box<dyn Reader>);

inventory::collect!(Registration);

/// Returns the department of a reader name (`Layout` for `Layout/LineLength`).
pub fn department(name: &str) -> &str {
    name.split_once('/')
        .map_or(name, |(department, _)| department)
}

/// The registered readers, instantiated once and sorted by name.
pub fn registry() -> &'static [Box<dyn Reader>] {
    static READERS: OnceLock<Vec<Box<dyn Reader>>> = OnceLock::new();
    READERS.get_or_init(all_readers)
}

/// Instantiates every registered reader, sorted by name.
pub fn all_readers() -> Vec<Box<dyn Reader>> {
    let mut readers: Vec<Box<dyn Reader>> = inventory::iter::<Registration>
        .into_iter()
        .map(|registration| (registration.0)())
        .collect();
    readers.sort_by_key(|reader| reader.name());
    readers
}

/// Names of every registered reader, sorted.
pub fn reader_names() -> Vec<&'static str> {
    registry().iter().map(|reader| reader.name()).collect()
}

/// The registered reader with exactly this name.
pub fn reader_named(name: &str) -> Option<&'static dyn Reader> {
    registry()
        .iter()
        .find(|reader| reader.name() == name)
        .map(AsRef::as_ref)
}

/// Resolves a reader name, a department or a bare reader name, case-insensitively.
///
/// `Layout` yields every `Layout/*` reader, `Layout/LineLength` and `LineLength` yield that
/// reader, and an unknown pattern yields nothing. The result is sorted.
pub fn find_reader(name_or_department: &str) -> Vec<&'static str> {
    let wanted = name_or_department.trim();
    let names = reader_names();
    let exact: Vec<&'static str> = names
        .iter()
        .copied()
        .filter(|name| name.eq_ignore_ascii_case(wanted))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    if wanted.contains('/') {
        return Vec::new();
    }
    let by_department: Vec<&'static str> = names
        .iter()
        .copied()
        .filter(|name| department(name).eq_ignore_ascii_case(wanted))
        .collect();
    if !by_department.is_empty() {
        return by_department;
    }
    names
        .into_iter()
        .filter(|name| {
            name.split_once('/')
                .is_some_and(|(_, short)| short.eq_ignore_ascii_case(wanted))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_sorted_and_unique() {
        let names = reader_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
        assert!(names.contains(&"Layout/TrailingWhitespace"));
        assert!(names.contains(&"Layout/LineLength"));
    }

    #[test]
    fn every_reader_is_well_formed() {
        for reader in registry() {
            let name = reader.name();
            let (department, short) = name.split_once('/').expect("Department/Name");
            assert!(
                ["Layout", "Style", "Lint", "Naming"].contains(&department),
                "{name}"
            );
            assert!(
                short.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
                "{name}"
            );
            assert!(
                reader.description().ends_with('.'),
                "{name} description must end with a period"
            );
        }
    }

    #[test]
    fn finds_readers_by_name_department_or_short_name() {
        assert_eq!(find_reader("Layout/LineLength"), vec!["Layout/LineLength"]);
        assert_eq!(find_reader("layout/linelength"), vec!["Layout/LineLength"]);
        assert_eq!(find_reader("LineLength"), vec!["Layout/LineLength"]);
        let layout = find_reader("layout");
        assert!(layout.contains(&"Layout/LineLength"));
        assert!(layout.contains(&"Layout/TrailingWhitespace"));
        assert!(find_reader("Nope/Nothing").is_empty());
        assert!(find_reader("Nothing").is_empty());
    }

    #[test]
    fn departments() {
        assert_eq!(department("Lint/Syntax"), "Lint");
        assert_eq!(department("Lint"), "Lint");
    }

    #[test]
    fn context_records_positions() {
        let source = Source::new("t.lua", "a\nbcd  \n");
        let config = ReaderConfig::default();
        let mut ctx = Context::new(&source, &config, "Layout/Test", Severity::Convention);
        ctx.add_offense_with_fix(5..7, "Msg.", vec![Edit::remove(5..7)]);
        let offenses = ctx.into_offenses();
        assert_eq!(offenses.len(), 1);
        let offense = &offenses[0];
        assert_eq!(
            (
                offense.line,
                offense.col,
                offense.last_line,
                offense.last_col
            ),
            (2, 4, 2, 5)
        );
        assert_eq!(offense.source_line, "bcd  ");
        assert!(offense.correctable());
    }
}
