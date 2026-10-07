//! Report formatters: `progress` (default), `offenses`, `files`, `quiet` and `json`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::Mutex;

use clap::ValueEnum;
use owo_colors::OwoColorize;

use crate::offense::{Offense, Severity};
use crate::runner::FileReport;

/// Output format selected with `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// A progress line of per-file symbols, then every offense with its source line.
    Progress,
    /// Offense counts per reader.
    Offenses,
    /// Paths of the files with offenses.
    Files,
    /// Every offense with its source line, without progress; nothing for a clean run.
    Quiet,
    /// Machine-readable JSON.
    Json,
}

/// Totals over a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Files inspected.
    pub files: usize,
    /// Offenses detected, corrected ones included.
    pub offenses: usize,
    /// Offenses corrected.
    pub corrected: usize,
    /// Offenses that could be corrected with `--fix` but were not.
    pub correctable: usize,
}

impl Summary {
    /// Computes the totals of `reports`.
    pub fn new(reports: &[FileReport]) -> Self {
        let offenses = || reports.iter().flat_map(|report| &report.offenses);
        Summary {
            files: reports.len(),
            offenses: offenses().count(),
            corrected: offenses().filter(|offense| offense.corrected).count(),
            correctable: offenses()
                .filter(|offense| offense.correctable() && !offense.corrected)
                .count(),
        }
    }

    /// The summary sentence, such as `3 files inspected, 1 offense detected, 1 offense autocorrectable`.
    pub fn line(&self) -> String {
        let mut line = format!("{} inspected, ", plural(self.files, "file"));
        if self.offenses == 0 {
            line.push_str("no offenses detected");
        } else {
            let _ = write!(line, "{} detected", plural(self.offenses, "offense"));
        }
        if self.corrected > 0 {
            let _ = write!(line, ", {} corrected", plural(self.corrected, "offense"));
        }
        if self.correctable > 0 {
            let _ = write!(
                line,
                ", {} autocorrectable",
                plural(self.correctable, "offense")
            );
        }
        line
    }
}

/// `count` followed by `word`, pluralised with an `s`.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// Colours `text` by `severity` (green for `None`) when `color` is set.
fn paint(text: &str, severity: Option<Severity>, color: bool) -> String {
    if !color {
        return text.to_owned();
    }
    match severity {
        None => text.green().to_string(),
        Some(Severity::Info | Severity::Refactor) => text.cyan().to_string(),
        Some(Severity::Convention) => text.yellow().to_string(),
        Some(Severity::Warning) => text.magenta().to_string(),
        Some(Severity::Error | Severity::Fatal) => text.red().to_string(),
    }
}

/// The progress symbol of a file: its worst severity's letter, `E` for an I/O error, or `.`.
pub fn progress_symbol(report: &FileReport, color: bool) -> String {
    if report.error.is_some() {
        return paint("E", Some(Severity::Error), color);
    }
    match report.offenses.iter().map(|offense| offense.severity).max() {
        Some(severity) => paint(&severity.symbol().to_string(), Some(severity), color),
        None => paint(".", None, color),
    }
}

/// Prints progress symbols in target order as files finish, from any thread.
pub struct Progress<W: Write + Send> {
    state: Mutex<ProgressState<W>>,
    color: bool,
}

/// Mutable part of [`Progress`].
struct ProgressState<W> {
    writer: W,
    next: usize,
    pending: Vec<Option<String>>,
}

impl<W: Write + Send> Progress<W> {
    /// Prints the `Inspecting N files` header and prepares for `total` files.
    pub fn start(mut writer: W, total: usize, color: bool) -> io::Result<Self> {
        writeln!(writer, "Inspecting {}", plural(total, "file"))?;
        writer.flush()?;
        Ok(Progress {
            state: Mutex::new(ProgressState {
                writer,
                next: 0,
                pending: vec![None; total],
            }),
            color,
        })
    }

    /// Records that file `index` finished; prints every symbol that is now in order.
    pub fn file_done(&self, index: usize, report: &FileReport) {
        let symbol = progress_symbol(report, self.color);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if let Some(slot) = state.pending.get_mut(index) {
            *slot = Some(symbol);
        }
        let mut ready = String::new();
        while let Some(Some(symbol)) = state.pending.get(state.next) {
            ready.push_str(symbol);
            state.next += 1;
        }
        if !ready.is_empty() {
            let _ = state.writer.write_all(ready.as_bytes());
            let _ = state.writer.flush();
        }
    }

    /// Ends the progress line and returns the writer.
    pub fn finish(self) -> io::Result<W> {
        let mut state = self
            .state
            .into_inner()
            .map_err(|_| io::Error::other("progress state poisoned"))?;
        writeln!(state.writer)?;
        Ok(state.writer)
    }
}

/// Renders the final report of a run in `format`.
pub fn render(format: Format, reports: &[FileReport], color: bool) -> String {
    match format {
        Format::Progress => render_clang(reports, color, true),
        Format::Quiet => render_clang(reports, color, false),
        Format::Offenses => render_counts(reports),
        Format::Files => render_files(reports),
        Format::Json => render_json(reports),
    }
}

/// Clang-style offense listing followed by the summary.
fn render_clang(reports: &[FileReport], color: bool, progress: bool) -> String {
    let summary = Summary::new(reports);
    let mut out = String::new();
    if summary.offenses == 0 {
        if progress {
            let _ = writeln!(out, "\n{}", summary.line());
        }
        return out;
    }
    if progress {
        out.push('\n');
    }
    out.push_str("Offenses:\n\n");
    for report in reports {
        for offense in &report.offenses {
            write_offense(&mut out, &report.path.display().to_string(), offense, color);
        }
    }
    let _ = writeln!(out, "\n{}", summary.line());
    out
}

/// Writes one clang-style offense: header, source line and carets.
fn write_offense(out: &mut String, path: &str, offense: &Offense, color: bool) {
    let marker = if offense.corrected {
        "[Corrected] "
    } else if offense.correctable() {
        "[Correctable] "
    } else {
        ""
    };
    let letter = paint(
        &format!("{}:", offense.severity.symbol()),
        Some(offense.severity),
        color,
    );
    let _ = writeln!(
        out,
        "{path}:{}:{}: {letter} {marker}{}: {}",
        offense.line, offense.col, offense.reader, offense.message
    );
    let line = offense.source_line.trim_end_matches('\r');
    if line.trim().is_empty() {
        return;
    }
    let mut start = (offense.col as usize).saturating_sub(1).min(line.len());
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let indent: String = line[..start]
        .chars()
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    let mut end = (start + offense.range.len()).min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let width = line[start..end].chars().count().max(1);
    let _ = writeln!(out, "{line}\n{indent}{}", "^".repeat(width));
}

/// Offense counts per reader, most frequent first, plus a total.
fn render_counts(reports: &[FileReport]) -> String {
    let mut counts: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    for offense in reports.iter().flat_map(|report| &report.offenses) {
        let entry = counts.entry(offense.reader).or_default();
        entry.0 += 1;
        entry.1 |= offense.correctable() || offense.corrected;
    }
    let total: usize = counts.values().map(|(count, _)| count).sum();
    let files = reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .count();
    let width = total.to_string().len() + 2;
    let mut rows: Vec<(&str, usize, bool)> = counts
        .into_iter()
        .map(|(reader, (count, correctable))| (reader, count, correctable))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut out = String::new();
    for (reader, count, correctable) in rows {
        let suffix = if correctable { " [Correctable]" } else { "" };
        let _ = writeln!(out, "{count:<width$}{reader}{suffix}");
    }
    let _ = writeln!(out, "--");
    let _ = writeln!(out, "{total:<width$}Total in {}", plural(files, "file"));
    out
}

/// One line per file with offenses.
fn render_files(reports: &[FileReport]) -> String {
    reports
        .iter()
        .filter(|report| !report.offenses.is_empty())
        .map(|report| format!("{}\n", report.path.display()))
        .collect()
}

/// Escapes `text` as a JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// JSON report in the spirit of RuboCop's `json` formatter.
fn render_json(reports: &[FileReport]) -> String {
    let summary = Summary::new(reports);
    let files: Vec<String> = reports
        .iter()
        .map(|report| {
            let offenses: Vec<String> = report
                .offenses
                .iter()
                .map(|offense| {
                    format!(
                        "{{\"severity\":{},\"message\":{},\"reader_name\":{},\"corrected\":{},\"correctable\":{},\
                         \"location\":{{\"start_line\":{},\"start_column\":{},\"last_line\":{},\"last_column\":{},\
                         \"length\":{},\"line\":{},\"column\":{}}}}}",
                        json_string(offense.severity.name()),
                        json_string(&offense.message),
                        json_string(offense.reader),
                        offense.corrected,
                        offense.correctable(),
                        offense.line,
                        offense.col,
                        offense.last_line,
                        offense.last_col,
                        offense.range.len(),
                        offense.line,
                        offense.col
                    )
                })
                .collect();
            let diff = report
                .diff
                .as_deref()
                .map_or_else(String::new, |diff| format!(",\"diff\":{}", json_string(diff)));
            format!(
                "{{\"path\":{},\"offenses\":[{}]{diff}}}",
                json_string(&report.path.display().to_string()),
                offenses.join(",")
            )
        })
        .collect();
    format!(
        "{{\"metadata\":{{\"proofreader_version\":{}}},\"files\":[{}],\"summary\":{{\"offense_count\":{},\
         \"target_file_count\":{},\"inspected_file_count\":{},\"corrected_count\":{},\"correctable_count\":{}}}}}\n",
        json_string(env!("CARGO_PKG_VERSION")),
        files.join(","),
        summary.offenses,
        summary.files,
        summary.files,
        summary.corrected,
        summary.correctable
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offense::Edit;
    use std::path::PathBuf;

    fn offense(line: u32, col: u32, len: usize, source_line: &str, correctable: bool) -> Offense {
        Offense {
            reader: "Layout/Thing",
            severity: Severity::Convention,
            message: "Bad thing.".to_owned(),
            range: 0..len,
            line,
            col,
            last_line: line,
            last_col: col,
            source_line: source_line.to_owned(),
            fix: correctable.then(|| vec![Edit::remove(0..len)]),
            corrected: false,
        }
    }

    fn report(path: &str, offenses: Vec<Offense>) -> FileReport {
        FileReport {
            path: PathBuf::from(path),
            offenses,
            diff: None,
            error: None,
        }
    }

    #[test]
    fn summary_pluralises() {
        assert_eq!(
            Summary::default().line(),
            "0 files inspected, no offenses detected"
        );
        let summary = Summary {
            files: 1,
            offenses: 1,
            corrected: 0,
            correctable: 1,
        };
        assert_eq!(
            summary.line(),
            "1 file inspected, 1 offense detected, 1 offense autocorrectable"
        );
        let summary = Summary {
            files: 3,
            offenses: 4,
            corrected: 2,
            correctable: 0,
        };
        assert_eq!(
            summary.line(),
            "3 files inspected, 4 offenses detected, 2 offenses corrected"
        );
    }

    #[test]
    fn clang_style_listing() {
        let reports = vec![
            report("a.lua", vec![offense(2, 5, 3, "  x = abc", true)]),
            report("b.lua", vec![]),
        ];
        let expected = "\nOffenses:\n\na.lua:2:5: C: [Correctable] Layout/Thing: Bad thing.\n  x = abc\n    ^^^\n\n\
                        2 files inspected, 1 offense detected, 1 offense autocorrectable\n";
        assert_eq!(render(Format::Progress, &reports, false), expected);
        assert_eq!(render(Format::Quiet, &reports, false), &expected[1..]);
    }

    #[test]
    fn clean_runs() {
        let reports = vec![report("a.lua", vec![])];
        assert_eq!(
            render(Format::Progress, &reports, false),
            "\n1 file inspected, no offenses detected\n"
        );
        assert_eq!(render(Format::Quiet, &reports, false), "");
        assert_eq!(render(Format::Files, &reports, false), "");
    }

    #[test]
    fn carets_follow_tabs_and_clip_to_the_line() {
        let mut out = String::new();
        write_offense(
            &mut out,
            "t.lua",
            &offense(1, 3, 50, "\tx\u{e9}y", false),
            false,
        );
        assert_eq!(
            out,
            "t.lua:1:3: C: Layout/Thing: Bad thing.\n\tx\u{e9}y\n\t ^^\n"
        );
        let mut blank = String::new();
        write_offense(&mut blank, "t.lua", &offense(1, 1, 2, "  ", true), false);
        assert_eq!(
            blank,
            "t.lua:1:1: C: [Correctable] Layout/Thing: Bad thing.\n"
        );
    }

    #[test]
    fn corrected_marker() {
        let mut fixed = offense(1, 1, 1, "x", true);
        fixed.corrected = true;
        let mut out = String::new();
        write_offense(&mut out, "t.lua", &fixed, false);
        assert!(out.starts_with("t.lua:1:1: C: [Corrected] Layout/Thing"));
    }

    #[test]
    fn offense_counts() {
        let mut other = offense(1, 1, 1, "x", false);
        other.reader = "Style/Other";
        let reports = vec![
            report("a.lua", vec![offense(1, 1, 1, "x", true), other.clone()]),
            report(
                "b.lua",
                vec![offense(1, 1, 1, "x", true), other.clone(), other],
            ),
            report("c.lua", vec![]),
        ];
        assert_eq!(
            render(Format::Offenses, &reports, false),
            "3  Style/Other\n2  Layout/Thing [Correctable]\n--\n5  Total in 2 files\n"
        );
        assert_eq!(render(Format::Files, &reports, false), "a.lua\nb.lua\n");
    }

    #[test]
    fn json_output() {
        let reports = vec![report("a\"b.lua", vec![offense(1, 2, 3, "x", false)])];
        let json = render(Format::Json, &reports, false);
        assert!(json.contains("\"path\":\"a\\\"b.lua\""));
        assert!(json.contains("\"reader_name\":\"Layout/Thing\""));
        assert!(json.contains("\"location\":{\"start_line\":1,\"start_column\":2"));
        assert!(json.contains("\"summary\":{\"offense_count\":1,\"target_file_count\":1"));
        assert_eq!(json_string("a\u{1}\n"), "\"a\\u0001\\n\"");
    }

    #[test]
    fn progress_prints_in_order() {
        let progress = Progress::start(Vec::new(), 3, false).expect("start");
        let clean = report("a.lua", vec![]);
        let dirty = report("b.lua", vec![offense(1, 1, 1, "x", false)]);
        progress.file_done(1, &dirty);
        progress.file_done(2, &clean);
        progress.file_done(0, &clean);
        let out = progress.finish().expect("finish");
        assert_eq!(
            String::from_utf8(out).expect("utf-8"),
            "Inspecting 3 files\n.C.\n"
        );
    }
}
