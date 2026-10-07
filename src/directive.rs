//! Inline directives: comments that switch readers off for part of a file, in the style of
//! ESLint's `eslint-disable` comments.
//!
//! | Comment | Suppresses offenses starting |
//! |---------|------------------------------|
//! | `-- proofreader-disable` | from the comment up to a matching `proofreader-enable` or the end of the file |
//! | `-- proofreader-enable` | nothing: ends an earlier `proofreader-disable` |
//! | `-- proofreader-disable-line` | on the line(s) of the comment |
//! | `-- proofreader-disable-next-line` | on the line after the comment |
//!
//! Every form works in any kind of comment (`--`, `//`, `--[[ ]]`, `/* */`) and may be followed
//! by a list of readers, separated by commas or spaces and resolved like `--only` (a full name,
//! a department or a bare name); without a list it applies to every reader. Anything after a
//! `--` standing on its own is a free-form explanation.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use crate::offense::Offense;
use crate::reader::find_reader;
use crate::source::Source;
use crate::token::Token;

/// The word every directive starts with.
const PREFIX: &str = "proofreader-";

/// What a directive does, with its spelling after [`PREFIX`]; longer spellings come first.
const ACTIONS: [(&str, Action); 4] = [
    ("disable-next-line", Action::DisableNextLine),
    ("disable-line", Action::DisableLine),
    ("disable", Action::Disable),
    ("enable", Action::Enable),
];

/// The kind of a directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Disable,
    Enable,
    DisableLine,
    DisableNextLine,
}

/// The readers a directive names; `None` stands for every reader.
type Readers = Option<BTreeSet<&'static str>>;

/// Whether a directive naming `readers` covers `reader`.
fn covers(readers: &Readers, reader: &str) -> bool {
    readers.as_ref().is_none_or(|names| names.contains(reader))
}

/// A `proofreader-disable` or `proofreader-enable` comment.
#[derive(Debug, Clone)]
struct Switch {
    /// Byte offset the directive takes effect at.
    offset: usize,
    /// Whether it disables (rather than re-enables) its readers.
    disable: bool,
    readers: Readers,
}

/// A `proofreader-disable-line` or `proofreader-disable-next-line` comment.
#[derive(Debug, Clone)]
struct LineRule {
    lines: RangeInclusive<u32>,
    readers: Readers,
}

/// Parses the text of a comment token as a directive.
fn parse_comment(text: &str) -> Option<(Action, Readers)> {
    let body = text
        .trim_start_matches(['-', '/', '*', '[', '='])
        .trim_end_matches([']', '=', '*', '/'])
        .trim();
    let rest = body.strip_prefix(PREFIX)?;
    let (action, rest) = ACTIONS
        .into_iter()
        .find_map(|(word, action)| rest.strip_prefix(word).map(|rest| (action, rest)))?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut names = rest
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .take_while(|word| word.len() < 2 || word.bytes().any(|byte| byte != b'-'))
        .peekable();
    let readers = names
        .peek()
        .is_some()
        .then(|| names.flat_map(find_reader).collect());
    Some((action, readers))
}

/// Where a `disable`/`enable` comment takes effect: at the start of its line when nothing but
/// whitespace precedes it there, so that it also covers the line's indentation, else at the
/// comment itself.
fn switch_offset(source: &Source, token: &Token) -> usize {
    let line_start = source.line_range(token.line).start;
    let before = &source.text[line_start..token.start];
    if before.trim_start_matches('\u{feff}').trim().is_empty() {
        line_start
    } else {
        token.start
    }
}

/// The inline directives of one source; see the module documentation.
#[derive(Debug, Clone, Default)]
pub struct Directives {
    /// In source order.
    switches: Vec<Switch>,
    line_rules: Vec<LineRule>,
}

impl Directives {
    /// Collects the directives in the comments of `source`.
    pub fn parse(source: &Source) -> Self {
        let mut directives = Directives::default();
        for token in source.tokens.iter().filter(|token| token.kind.is_comment()) {
            let Some((action, readers)) = parse_comment(source.text_of(token)) else {
                continue;
            };
            let lines = match action {
                Action::Disable | Action::Enable => {
                    directives.switches.push(Switch {
                        offset: switch_offset(source, token),
                        disable: action == Action::Disable,
                        readers,
                    });
                    continue;
                }
                Action::DisableLine => token.line..=token.end_line,
                Action::DisableNextLine => token.end_line + 1..=token.end_line + 1,
            };
            directives.line_rules.push(LineRule { lines, readers });
        }
        directives
    }

    /// Whether `source` holds no directive.
    pub fn is_empty(&self) -> bool {
        self.switches.is_empty() && self.line_rules.is_empty()
    }

    /// Whether a directive switches off `offense`, judged by where the offense starts.
    ///
    /// Among the `disable` and `enable` comments, the last one before the offense that covers
    /// its reader decides.
    pub fn suppresses(&self, offense: &Offense) -> bool {
        self.line_rules
            .iter()
            .any(|rule| rule.lines.contains(&offense.line) && covers(&rule.readers, offense.reader))
            || self
                .switches
                .iter()
                .rev()
                .find(|switch| {
                    switch.offset <= offense.range.start && covers(&switch.readers, offense.reader)
                })
                .is_some_and(|switch| switch.disable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::inspect;

    const NOT: &str = "Style/Not";

    /// Lines of the offenses `reader` reports for `src`.
    fn lines(reader: &str, src: &str) -> Vec<u32> {
        inspect(reader, src)
            .iter()
            .map(|offense| offense.line)
            .collect()
    }

    #[test]
    fn disable_line_covers_its_own_line() {
        let src = "x = not a\ny = not b -- proofreader-disable-line\nz = not c\n";
        assert_eq!(lines(NOT, src), vec![1, 3]);
    }

    #[test]
    fn disable_next_line_covers_the_following_line() {
        let src = "-- proofreader-disable-next-line\nx = not a\ny = not b\n";
        assert_eq!(lines(NOT, src), vec![3]);
        let src = "--[[ proofreader-disable-next-line\n  Style/Not ]]\nx = not a\ny = not b\n";
        assert_eq!(lines(NOT, src), vec![4]);
    }

    #[test]
    fn disable_lasts_until_enable_or_the_end_of_the_file() {
        let src =
            "x = not a\n-- proofreader-disable\ny = not b\n-- proofreader-enable\nz = not c\n";
        assert_eq!(lines(NOT, src), vec![1, 5]);
        let src = "x = not a\n-- proofreader-disable\ny = not b\nz = not c\n";
        assert_eq!(lines(NOT, src), vec![1]);
        let src =
            "x = not a /* proofreader-disable */ y = not b /* proofreader-enable */ z = not c\n";
        let columns: Vec<u32> = inspect(NOT, src)
            .iter()
            .map(|offense| offense.col)
            .collect();
        assert_eq!(columns, vec![5, 76]);
    }

    #[test]
    fn readers_can_be_named() {
        for names in [
            "Style/Not",
            "style/not",
            "Not",
            "Style",
            "Layout/LineLength, Style/Not",
            "Layout/LineLength Style/Not",
            "Nope,Style/Not",
        ] {
            let src = format!("x = not a -- proofreader-disable-line {names}\n");
            assert_eq!(lines(NOT, &src), Vec::<u32>::new(), "{names}");
        }
        for names in ["Layout/LineLength", "Layout", "Nope", "Style/Nope"] {
            let src = format!("x = not a -- proofreader-disable-line {names}\n");
            assert_eq!(lines(NOT, &src), vec![1], "{names}");
        }
    }

    #[test]
    fn enable_can_restore_single_readers() {
        let src =
            "-- proofreader-disable\nx = not a \n-- proofreader-enable Style/Not\ny = not b \n";
        assert_eq!(lines(NOT, src), vec![4]);
        assert_eq!(lines("Layout/TrailingWhitespace", src), Vec::<u32>::new());
        let src =
            "-- proofreader-disable Style/Not\nx = not a \n-- proofreader-enable\ny = not b \n";
        assert_eq!(lines(NOT, src), vec![4]);
        assert_eq!(lines("Layout/TrailingWhitespace", src), vec![2, 4]);
    }

    #[test]
    fn explanations_follow_a_double_dash() {
        for comment in [
            "-- proofreader-disable-line Style/Not -- legacy API",
            "-- proofreader-disable-line -- legacy API",
            "-- proofreader-disable-line Style/Not --- Layout/LineLength",
        ] {
            assert_eq!(
                lines(NOT, &format!("x = not a {comment}\n")),
                Vec::<u32>::new(),
                "{comment}"
            );
        }
        let src = "x = not a -- proofreader-disable-line Layout/LineLength -- Style/Not\n";
        assert_eq!(lines(NOT, src), vec![1]);
    }

    #[test]
    fn every_comment_form_works() {
        for comment in [
            "--proofreader-disable-line",
            "--- proofreader-disable-line",
            "// proofreader-disable-line",
            "--[[ proofreader-disable-line ]]",
            "--[==[proofreader-disable-line Style/Not]==]",
            "/* proofreader-disable-line */",
            "/*proofreader-disable-line Style/Not*/",
        ] {
            assert_eq!(
                lines(NOT, &format!("x = not a {comment}\n")),
                Vec::<u32>::new(),
                "{comment}"
            );
        }
    }

    #[test]
    fn other_comments_and_strings_are_not_directives() {
        for tail in [
            "-- proofreader-disabled",
            "-- proofreader-disable-lines",
            "-- proofreader-disable-line: Style/Not",
            "-- see proofreader-disable-line",
            "-- proofreader disable-line",
            "..'-- proofreader-disable-line'",
        ] {
            assert_eq!(
                lines(NOT, &format!("x = not a {tail}\n")),
                vec![1],
                "{tail}"
            );
        }
        assert!(
            Directives::parse(&Source::new(
                "x.lua",
                "-- note\nx = '-- proofreader-disable'\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn a_directive_on_its_own_line_takes_effect_at_the_line_start() {
        let source = Source::new(
            "x.lua",
            "a()\n  -- proofreader-disable\nb() -- proofreader-enable\n",
        );
        let directives = Directives::parse(&source);
        let offsets: Vec<usize> = directives
            .switches
            .iter()
            .map(|switch| switch.offset)
            .collect();
        assert_eq!(offsets, vec![4, 33]);
        let with_bom = Source::new("x.lua", "\u{feff}-- proofreader-disable\n");
        assert_eq!(Directives::parse(&with_bom).switches[0].offset, 0);
    }
}
