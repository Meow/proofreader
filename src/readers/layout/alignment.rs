//! Alignment checks shared by `Layout/SpaceAroundOperators` and `Layout/ExtraSpacing`.
//!
//! Columns are counted in characters, so that alignment after non-ASCII text matches what an
//! editor shows.

use crate::source::Source;
use crate::token::{Token, TokenKind};

/// The character column (0-based) at which `token` starts on its first line.
pub fn char_column(source: &Source, token: &Token) -> usize {
    let line_start = source.line_range(token.line).start;
    source
        .text
        .get(line_start..token.start)
        .map_or(0, |prefix| prefix.chars().count())
}

/// Whether line `line` has no code token other than comments starting on it, so that alignment
/// checks look past it (blank lines, comment-only lines and lines inside long strings).
fn is_skipped(source: &Source, line: u32) -> bool {
    source
        .code_tokens_on_line(line)
        .all(|token| token.kind.is_comment())
}

/// The lines above (`up`) or below `line`, nearest first, that alignment checks consider.
fn candidate_lines(source: &Source, line: u32, up: bool) -> Box<dyn Iterator<Item = u32> + '_> {
    let lines: Box<dyn Iterator<Item = u32>> = if up {
        Box::new((1..line).rev())
    } else {
        Box::new(line + 1..=source.line_count())
    };
    Box::new(lines.filter(move |&n| !is_skipped(source, n)))
}

/// Whether line `line` has a code token that starts at character column `column` and satisfies
/// `wanted`.
fn has_token_at(
    source: &Source,
    line: u32,
    column: usize,
    wanted: &impl Fn(&Token) -> bool,
) -> bool {
    source
        .code_tokens_on_line(line)
        .any(|token| char_column(source, token) == column && wanted(token))
}

/// Whether a code token satisfying `wanted` starts at the same column as `token` on the nearest
/// line above or below it, looking past blank and comment-only lines.
///
/// As in RuboCop, when neither of those lines lines up, the nearest lines above and below with
/// the same indentation as the line of `token` are tried as well.
pub fn aligned_with_adjacent_line(
    source: &Source,
    token: &Token,
    wanted: impl Fn(&Token) -> bool,
) -> bool {
    let column = char_column(source, token);
    let indentation = source.indentation(token.line);
    let check = |line: Option<u32>| line.is_some_and(|n| has_token_at(source, n, column, &wanted));
    [true, false]
        .into_iter()
        .any(|up| check(candidate_lines(source, token.line, up).next()))
        || [true, false].into_iter().any(|up| {
            check(
                candidate_lines(source, token.line, up)
                    .find(|&n| source.indentation(n) == indentation),
            )
        })
}

/// Whether the operator `token` lines up with the same operator on an adjacent line (see
/// [`aligned_with_adjacent_line`]).
pub fn is_aligned_operator(source: &Source, token: &Token) -> bool {
    let text = source.text_of(token);
    aligned_with_adjacent_line(source, token, |other| source.text_of(other) == text)
}

/// One `=` taking part in a group of consecutive assignment lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignmentLine<'a> {
    /// The `=` token.
    pub equals: &'a Token,
    /// The code token directly before the `=`.
    pub before: &'a Token,
    /// The column the `=` would have with exactly one space before it.
    pub minimum_column: usize,
}

/// The first `=` of line `line` outside any brackets opened on the line, when the line starts a
/// statement or table field with a name, a bracketed key or `local`, together with the token
/// before it on the same line.
fn assignment_on_line(source: &Source, line: u32) -> Option<AssignmentLine<'_>> {
    let first = source.first_code_token_on_line(line)?;
    if !matches!(
        first.kind,
        TokenKind::Name | TokenKind::Local | TokenKind::LBracket
    ) {
        return None;
    }
    let tokens: Vec<&Token> = source.code_tokens_on_line(line).collect();
    let mut depth = 0usize;
    let mut position = None;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => depth += 1,
            TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                depth = depth.checked_sub(1)?;
            }
            TokenKind::Assign if depth == 0 => {
                position = Some(index);
                break;
            }
            _ => {}
        }
    }
    let position = position?;
    let before = *tokens.get(position.checked_sub(1)?)?;
    let equals = tokens[position];
    let before_end = source
        .text
        .get(source.line_range(line).start..before.end)
        .map_or(0, |prefix| prefix.chars().count());
    Some(AssignmentLine {
        equals,
        before,
        minimum_column: before_end + 1,
    })
}

/// Groups of two or more assignment lines that follow each other directly (no blank or other
/// line in between) and share their indentation, for `ForceEqualSignAlignment`.
pub fn assignment_groups(source: &Source) -> Vec<Vec<AssignmentLine<'_>>> {
    let skipped = source.lines_inside_multiline_tokens();
    let mut groups = Vec::new();
    let mut current: Vec<AssignmentLine> = Vec::new();
    let mut indentation = "";
    for line in 1..=source.line_count() {
        let assignment = if skipped.contains(&line) {
            None
        } else {
            assignment_on_line(source, line)
        };
        match assignment {
            Some(found) if current.is_empty() || source.indentation(line) == indentation => {
                indentation = source.indentation(line);
                current.push(found);
            }
            Some(found) => {
                groups.push(std::mem::take(&mut current));
                indentation = source.indentation(line);
                current.push(found);
            }
            None => groups.push(std::mem::take(&mut current)),
        }
    }
    groups.push(current);
    groups.retain(|group| group.len() > 1);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_at(source: &Source, line: u32, text: &str) -> Token {
        *source
            .code_tokens_on_line(line)
            .find(|token| source.text_of(token) == text)
            .expect("token")
    }

    #[test]
    fn finds_aligned_operators() {
        let source = Source::new("t.lua", "a   = 1\n\nbcd = 2\nx  = 3\n");
        assert!(is_aligned_operator(&source, &token_at(&source, 1, "=")));
        assert!(is_aligned_operator(&source, &token_at(&source, 3, "=")));
        assert!(!is_aligned_operator(&source, &token_at(&source, 4, "=")));
        let source = Source::new("t.lua", "x = a  or b\ny = cc or d\n");
        assert!(is_aligned_operator(&source, &token_at(&source, 1, "or")));
    }

    #[test]
    fn looks_past_comment_lines() {
        let source = Source::new("t.lua", "a   = 1\n\n-- note\n--[[ x\ny ]]\nbcd = 2\n");
        assert!(is_aligned_operator(&source, &token_at(&source, 1, "=")));
    }

    #[test]
    fn falls_back_to_lines_with_the_same_indentation() {
        let source = Source::new("t.lua", "a   = 1\nt = {\n  1\n}\nbcd = 2\n");
        assert!(!is_aligned_operator(&source, &token_at(&source, 1, "=")));
        let source = Source::new("t.lua", "a   = f(\n  1)\nbcd = 2\n");
        assert!(is_aligned_operator(&source, &token_at(&source, 1, "=")));
    }

    #[test]
    fn counts_columns_in_characters() {
        let source = Source::new("t.lua", "t['é'] = 1\nt['e'] = 2\n");
        assert!(is_aligned_operator(&source, &token_at(&source, 1, "=")));
        assert_eq!(char_column(&source, &token_at(&source, 1, "=")), 7);
    }

    #[test]
    fn groups_consecutive_assignments() {
        let source = Source::new(
            "t.lua",
            "a = 1\nbb = 2\n\nc = 3\n  d = 4\n  ee = 5\nf(x)\ng = 1\nf({ h = 1 })\n",
        );
        let groups = assignment_groups(&source);
        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2]);
        assert_eq!(groups[0][1].minimum_column, 3);
    }
}
