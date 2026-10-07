//! Offenses reported by readers, their severities and the edits that fix them.

use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use thiserror::Error;

/// How serious an offense is, from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational only.
    Info,
    /// Code that could be refactored.
    Refactor,
    /// A style convention violation.
    Convention,
    /// Probably a bug.
    Warning,
    /// Certainly a bug.
    Error,
    /// The file cannot be processed.
    Fatal,
}

impl Severity {
    /// All severities in increasing order.
    pub const ALL: [Severity; 6] = [
        Severity::Info,
        Severity::Refactor,
        Severity::Convention,
        Severity::Warning,
        Severity::Error,
        Severity::Fatal,
    ];

    /// The one-letter code used in reports: `I`, `R`, `C`, `W`, `E` or `F`.
    pub fn symbol(self) -> char {
        match self {
            Severity::Info => 'I',
            Severity::Refactor => 'R',
            Severity::Convention => 'C',
            Severity::Warning => 'W',
            Severity::Error => 'E',
            Severity::Fatal => 'F',
        }
    }

    /// The lowercase name used in configuration files and JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Refactor => "refactor",
            Severity::Convention => "convention",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Error returned when parsing an unknown severity name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown severity `{0}` (expected info, refactor, convention, warning, error or fatal)")]
pub struct ParseSeverityError(pub String);

impl FromStr for Severity {
    type Err = ParseSeverityError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Severity::ALL
            .into_iter()
            .find(|severity| severity.name().eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| ParseSeverityError(name.to_owned()))
    }
}

/// A text replacement; an empty range is an insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The byte range to replace.
    pub range: Range<usize>,
    /// The text to put in its place.
    pub replacement: String,
}

impl Edit {
    /// Replaces `range` with `text`.
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Edit {
            range,
            replacement: text.into(),
        }
    }

    /// Removes `range`.
    pub fn remove(range: Range<usize>) -> Self {
        Edit::replace(range, "")
    }

    /// Inserts `text` at byte `offset`.
    pub fn insert(offset: usize, text: impl Into<String>) -> Self {
        Edit::replace(offset..offset, text)
    }
}

/// A problem found by a reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offense {
    /// Name of the reader that reported it, such as `Layout/LineLength`.
    pub reader: &'static str,
    /// How serious it is.
    pub severity: Severity,
    /// The message shown to the user.
    pub message: String,
    /// The byte range the offense covers.
    pub range: Range<usize>,
    /// 1-based line of the start of the range.
    pub line: u32,
    /// 1-based byte column of the start of the range.
    pub col: u32,
    /// 1-based line of the last byte of the range.
    pub last_line: u32,
    /// 1-based byte column of the last byte of the range.
    pub last_col: u32,
    /// Text of the line holding the start of the range, at detection time.
    pub source_line: String,
    /// Edits that fix the offense, when the reader can correct it.
    pub fix: Option<Vec<Edit>>,
    /// Whether the fix was applied.
    pub corrected: bool,
}

impl Offense {
    /// Whether the offense comes with a fix.
    pub fn correctable(&self) -> bool {
        self.fix.is_some()
    }

    /// Identity used to de-duplicate offenses across autocorrect passes.
    pub fn key(&self) -> (&'static str, u32, u32, &str) {
        (self.reader, self.line, self.col, &self.message)
    }

    /// Attaches a fix to the offense.
    pub fn with_fix(&mut self, edits: Vec<Edit>) -> &mut Self {
        self.fix = Some(edits);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_are_ordered_and_parse() {
        assert!(Severity::Info < Severity::Convention);
        assert!(Severity::Warning < Severity::Fatal);
        assert_eq!("warning".parse(), Ok(Severity::Warning));
        assert_eq!("Convention".parse(), Ok(Severity::Convention));
        assert!("loud".parse::<Severity>().is_err());
        let symbols: String = Severity::ALL
            .iter()
            .map(|severity| severity.symbol())
            .collect();
        assert_eq!(symbols, "IRCWEF");
    }

    #[test]
    fn edit_constructors() {
        assert_eq!(Edit::insert(3, "x").range, 3..3);
        assert_eq!(Edit::remove(1..2).replacement, "");
    }
}
