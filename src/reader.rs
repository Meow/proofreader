//! The [`Reader`] trait implemented by every check, the [`Context`] it reports into, and the
//! registry that collects readers through `inventory`.

use std::ops::Range;
use std::sync::OnceLock;

use yaml_rust2::Yaml;

use crate::config::{Config, ReaderConfig};
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

    /// Severity used unless configured otherwise: `Warning` for `Lint/*`, `Performance/*` and
    /// `Security/*`, `Convention` otherwise.
    fn default_severity(&self) -> Severity {
        if matches!(department(self.name()), "Lint" | "Performance" | "Security") {
            Severity::Warning
        } else {
            Severity::Convention
        }
    }

    /// Whether the reader runs unless configured otherwise; readers that are noisy on most code
    /// bases are disabled by default and enabled with `Enabled: true`.
    fn default_enabled(&self) -> bool {
        true
    }

    /// Reader-specific options and their defaults, in display order.
    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        Vec::new()
    }

    /// The values each enumerated option (`EnforcedStyle` and the like) accepts; the
    /// configuration warns about any other value.
    fn allowed_values(&self) -> Vec<(&'static str, &'static [&'static str])> {
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
    /// The configuration of every reader for this file, when known.
    configuration: Option<&'a Config>,
    offenses: Vec<Offense>,
    reader: &'static str,
    severity: Severity,
    /// The byte offset and character column of the last column computed, which later columns
    /// on the same line count on from.
    cursor: (usize, u32),
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
            configuration: None,
            offenses: Vec::new(),
            reader,
            severity,
            cursor: (0, 1),
        }
    }

    /// The 1-based character column of byte `offset`, counted from the start of its line or
    /// from the last column computed when that lies earlier on the same line, so offenses
    /// reported in order along a long line take time in proportion to the line.
    fn char_column(&mut self, line: u32, offset: usize) -> u32 {
        let text = &self.source.text;
        let line_start = self.source.line_range(line).start;
        let mut end = offset.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let (from, column) = if (line_start..=end).contains(&self.cursor.0) {
            self.cursor
        } else {
            (line_start, 1)
        };
        let counted = u32::try_from(text[from..end].chars().count()).unwrap_or(u32::MAX);
        let column = column.saturating_add(counted);
        self.cursor = (end, column);
        column
    }

    /// Lets the reader see the configuration of the other readers for this file.
    pub fn with_configuration(mut self, configuration: &'a Config) -> Self {
        self.configuration = Some(configuration);
        self
    }

    /// The effective configuration of the reader `name` for this file, when the configuration of
    /// the other readers is known.
    pub fn reader_config(&self, name: &str) -> Option<&'a ReaderConfig> {
        self.configuration
            .map(|configuration| configuration.reader(name))
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
        let char_col = self.char_column(line, range.start);
        let last_char_col = self.char_column(last_line, last);
        let (excerpt, excerpt_start) = self.source.excerpt(line, range.start);
        self.offenses.push(Offense {
            reader: self.reader,
            severity: self.severity,
            message: message.into(),
            range,
            line,
            col,
            char_col,
            last_line,
            last_col,
            last_char_col,
            source_line: excerpt.to_owned(),
            excerpt_start: u32::try_from(excerpt_start).unwrap_or(u32::MAX),
            fix: None,
            unsafe_fix: false,
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

    /// Mapping option `key` of the running reader, as `(key, value)` pairs in order.
    pub fn option_str_map(&self, key: &str) -> Vec<(String, String)> {
        self.config.get_str_map(key)
    }
}

/// Registers a reader; submit one per reader with `inventory::submit!`.
pub struct Registration(pub fn() -> Box<dyn Reader>);

inventory::collect!(Registration);

/// The departments readers belong to; a configuration section named after a department applies
/// to every reader of it.
pub const DEPARTMENTS: [&str; 6] = [
    "Layout",
    "Lint",
    "Naming",
    "Performance",
    "Security",
    "Style",
];

/// Options every reader of a department reads from the department's section, with their
/// defaults; submit one per department that has such options with `inventory::submit!`.
pub struct SharedOptions {
    /// The department.
    pub department: &'static str,
    /// The options and their defaults, in display order.
    pub defaults: fn() -> Vec<(&'static str, Yaml)>,
}

inventory::collect!(SharedOptions);

/// The shared options of `department` with their defaults, in display order.
pub fn department_options(department: &str) -> Vec<(&'static str, Yaml)> {
    inventory::iter::<SharedOptions>
        .into_iter()
        .filter(|shared| shared.department == department)
        .flat_map(|shared| (shared.defaults)())
        .collect()
}

/// A YAML list of strings, for option defaults.
pub fn yaml_strings(items: &[&str]) -> Yaml {
    Yaml::Array(
        items
            .iter()
            .map(|item| Yaml::String((*item).to_owned()))
            .collect(),
    )
}

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

/// The edit distance between `a` and `b`, ignoring case: insertions, deletions, substitutions
/// and swaps of adjacent characters each count one.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().flat_map(char::to_lowercase).collect();
    let b: Vec<char> = b.chars().flat_map(char::to_lowercase).collect();
    let mut rows = vec![(0..=b.len()).collect::<Vec<usize>>()];
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let substitution = rows[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]);
            row[j] = substitution.min(rows[i - 1][j] + 1).min(row[j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(rows[i - 2][j - 2] + 1);
            }
        }
        rows.push(row);
    }
    rows[a.len()][b.len()]
}

/// The candidate nearest to `word`, ignoring case, when it is close enough to be what `word`
/// misspells: at most a third of the length of `word` (and at least one) edits away. Each
/// candidate is `(shown, compared)`: `compared` is matched against `word` and `shown` returned.
pub fn closest<'a>(
    word: &str,
    candidates: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<&'a str> {
    let limit = (word.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|(shown, compared)| (distance(word, compared), shown))
        .filter(|(edits, _)| *edits <= limit)
        .min_by_key(|(edits, _)| *edits)
        .map(|(_, shown)| shown)
}

/// The reader or department nearest to the misspelled `name`, compared with full names,
/// departments and bare reader names; see [`closest`].
pub fn suggest_reader(name: &str) -> Option<&'static str> {
    let names = reader_names();
    let candidates = names
        .iter()
        .map(|name| (*name, *name))
        .chain(
            DEPARTMENTS
                .iter()
                .map(|department| (*department, *department)),
        )
        .chain(
            names
                .iter()
                .filter_map(|name| name.split_once('/').map(|(_, short)| (*name, short))),
        );
    closest(name.trim(), candidates)
}

/// `; did you mean X?` for a suggestion `X`, or nothing.
pub fn did_you_mean(suggestion: Option<&str>) -> String {
    suggestion.map_or_else(String::new, |name| format!("; did you mean {name}?"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_nearest_names() {
        assert_eq!(suggest_reader("Style/Nott"), Some("Style/Not"));
        assert_eq!(
            suggest_reader("style/stringliteral"),
            Some("Style/StringLiterals")
        );
        assert_eq!(suggest_reader("Stlye"), Some("Style"));
        assert_eq!(suggest_reader("LineLenght"), Some("Layout/LineLength"));
        assert_eq!(suggest_reader("Completely/Different"), None);
        assert_eq!(
            closest("EnforcedStlye", [("EnforcedStyle", "EnforcedStyle")]),
            Some("EnforcedStyle")
        );
        assert_eq!(closest("ab", [("xy", "xy")]), None);
        assert_eq!(did_you_mean(Some("X")), "; did you mean X?");
        assert_eq!(did_you_mean(None), "");
    }

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
            assert!(DEPARTMENTS.contains(&department), "{name}");
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

    #[test]
    fn context_records_character_columns_and_excerpts() {
        let text = format!("é = 1\n{}x", "é".repeat(500));
        let source = Source::new("t.lua", text.as_str());
        let config = ReaderConfig::default();
        let mut ctx = Context::new(&source, &config, "Layout/Test", Severity::Convention);
        ctx.add_offense(4..5, "Msg.");
        let x = text.len() - 1;
        ctx.add_offense(x - 4..x + 1, "Msg.");
        ctx.add_offense(9..11, "Msg.");
        let offenses = ctx.into_offenses();
        assert_eq!(
            (
                offenses[0].col,
                offenses[0].char_col,
                offenses[0].last_char_col
            ),
            (5, 4, 4)
        );
        assert_eq!(
            (
                offenses[1].col,
                offenses[1].char_col,
                offenses[1].last_char_col
            ),
            (997, 499, 501)
        );
        assert!(offenses[1].source_line.ends_with("éééx"));
        assert_eq!(offenses[1].source_line.chars().count(), 103);
        assert_eq!(
            offenses[1].excerpt_start as usize,
            2 * (499 - 1 - crate::offense::EXCERPT_CHARS / 3)
        );
        assert_eq!((offenses[2].char_col, offenses[2].last_char_col), (2, 2));
        assert_eq!(offenses[2].excerpt_start, 0);
        assert_eq!(offenses[2].source_line.chars().count(), 300);
    }
}
