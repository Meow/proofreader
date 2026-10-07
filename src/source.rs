//! A processed source file: text, line table, tokens and position helpers for readers.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use crate::lexer::lex;
use crate::token::{Token, TokenKind};

/// A lexed source file. Line numbers are 1-based `u32`s everywhere, matching [`Token::line`].
#[derive(Debug, Clone)]
pub struct Source {
    /// Display path of the file, relative to the working directory when possible.
    pub path: PathBuf,
    /// The full text of the file.
    pub text: String,
    lines: Vec<Range<usize>>,
    /// Every token, trivia included, ending with `Eof`.
    pub tokens: Vec<Token>,
    code: Vec<usize>,
}

impl Source {
    /// Lexes `text` and builds the line table.
    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        let text = text.into();
        let tokens = lex(&text);
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                lines.push(start..index);
                start = index + 1;
            }
        }
        lines.push(start..text.len());
        let code = tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| !token.is_trivia())
            .map(|(index, _)| index)
            .collect();
        Source {
            path: path.into(),
            text,
            lines,
            tokens,
            code,
        }
    }

    /// Number of lines, with `str::split('\n')` semantics: a trailing `\n` yields a final empty line.
    pub fn line_count(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }

    /// Text of line `n` without its `\n` (a `\r` before it is kept); empty when out of range.
    pub fn line(&self, n: u32) -> &str {
        &self.text[self.line_range(n)]
    }

    /// Iterates over `(line number, line text)` pairs.
    pub fn lines(&self) -> impl Iterator<Item = (u32, &str)> {
        (1..=self.line_count()).map(|n| (n, self.line(n)))
    }

    /// Byte range of line `n` without its `\n`; an empty range at the end of the text when out of range.
    pub fn line_range(&self, n: u32) -> Range<usize> {
        n.checked_sub(1)
            .and_then(|index| self.lines.get(index as usize))
            .cloned()
            .unwrap_or(self.text.len()..self.text.len())
    }

    /// Converts a byte offset to a 1-based `(line, byte column)` pair.
    pub fn position(&self, offset: usize) -> (u32, u32) {
        let index = self
            .lines
            .partition_point(|range| range.start <= offset)
            .saturating_sub(1);
        let start = self.lines.get(index).map_or(0, |range| range.start);
        let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let col = u32::try_from(offset.saturating_sub(start) + 1).unwrap_or(u32::MAX);
        (line, col)
    }

    /// Converts a 1-based `(line, byte column)` pair to a byte offset, clamped to the text.
    pub fn offset(&self, line: u32, col: u32) -> usize {
        let start = self.line_range(line).start;
        (start + col.saturating_sub(1) as usize).min(self.text.len())
    }

    /// Whether line `n` is empty or contains only whitespace.
    pub fn is_blank(&self, n: u32) -> bool {
        self.line(n).trim().is_empty()
    }

    /// The leading spaces and tabs of line `n`.
    pub fn indentation(&self, n: u32) -> &str {
        let line = self.line(n);
        let width = line.len() - line.trim_start_matches([' ', '\t']).len();
        &line[..width]
    }

    /// Iterates over the non-trivia tokens (comments, `;` and `Eof` included).
    pub fn code_tokens(&self) -> impl Iterator<Item = &Token> {
        self.code.iter().map(|&index| &self.tokens[index])
    }

    /// Indexes into [`Source::tokens`] of the non-trivia tokens; position `i` here is "code index" `i`.
    pub fn code_token_indexes(&self) -> &[usize] {
        &self.code
    }

    /// The non-trivia token with the given code index.
    pub fn code_token(&self, code_index: usize) -> Option<&Token> {
        self.code.get(code_index).map(|&index| &self.tokens[index])
    }

    /// The non-trivia token before the given code index.
    pub fn prev_code(&self, code_index: usize) -> Option<&Token> {
        code_index
            .checked_sub(1)
            .and_then(|index| self.code_token(index))
    }

    /// The non-trivia token after the given code index.
    pub fn next_code(&self, code_index: usize) -> Option<&Token> {
        self.code_token(code_index + 1)
    }

    /// The text of `token` in this source.
    pub fn text_of(&self, token: &Token) -> &str {
        token.text(&self.text)
    }

    /// Index into [`Source::tokens`] of the token covering byte `offset`.
    pub fn token_index_at(&self, offset: usize) -> Option<usize> {
        let index = self.tokens.partition_point(|token| token.end <= offset);
        self.tokens
            .get(index)
            .filter(|token| token.start <= offset && offset < token.end)
            .map(|_| index)
    }

    /// The token (trivia included) covering byte `offset`.
    pub fn token_at(&self, offset: usize) -> Option<&Token> {
        self.token_index_at(offset).map(|index| &self.tokens[index])
    }

    /// All tokens, trivia included, that start on line `n`.
    pub fn tokens_on_line(&self, n: u32) -> &[Token] {
        let start = self.tokens.partition_point(|token| token.line < n);
        let end = self.tokens.partition_point(|token| token.line <= n);
        &self.tokens[start..end]
    }

    /// The non-trivia tokens starting on line `n`, never including `Eof`.
    pub fn code_tokens_on_line(&self, n: u32) -> impl Iterator<Item = &Token> {
        self.tokens_on_line(n)
            .iter()
            .filter(|token| !token.is_trivia() && token.kind != TokenKind::Eof)
    }

    /// The first non-trivia token starting on line `n` (never `Eof`).
    pub fn first_code_token_on_line(&self, n: u32) -> Option<&Token> {
        self.code_tokens_on_line(n).next()
    }

    /// The last non-trivia token ending on line `n` (never `Eof`); multi-line tokens count for
    /// the line they end on.
    pub fn last_code_token_on_line(&self, n: u32) -> Option<&Token> {
        let start = self.tokens.partition_point(|token| token.end_line < n);
        let end = self.tokens.partition_point(|token| token.end_line <= n);
        self.tokens[start..end]
            .iter()
            .rev()
            .find(|token| !token.is_trivia() && token.kind != TokenKind::Eof)
    }

    /// Whether byte `offset` lies inside a string or comment token.
    pub fn in_string_or_comment(&self, offset: usize) -> bool {
        self.token_at(offset)
            .is_some_and(|token| token.kind.is_string() || token.kind.is_comment())
    }

    /// Lines strictly inside multi-line strings and comments: every line after the token's first
    /// line, up to and including the line it ends on.
    pub fn lines_inside_multiline_tokens(&self) -> HashSet<u32> {
        self.tokens
            .iter()
            .filter(|token| {
                token.is_multiline() && (token.kind.is_string() || token.kind.is_comment())
            })
            .flat_map(|token| token.line + 1..=token.end_line)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::new("test.lua", text)
    }

    #[test]
    fn lines_follow_split_semantics() {
        let src = source("a\nbc\n");
        assert_eq!(src.line_count(), 3);
        assert_eq!(src.line(1), "a");
        assert_eq!(src.line(2), "bc");
        assert_eq!(src.line(3), "");
        assert_eq!(src.line(0), "");
        assert_eq!(src.line(4), "");
        assert_eq!(source("").line_count(), 1);
        assert_eq!(source("x").line_count(), 1);
    }

    #[test]
    fn carriage_returns_stay_in_the_line() {
        let src = source("a\r\nb");
        assert_eq!(src.line(1), "a\r");
        assert_eq!(src.line_range(1), 0..2);
        assert_eq!(src.line_range(2), 3..4);
    }

    #[test]
    fn positions_and_offsets_round_trip() {
        let src = source("ab\n\ncde\n");
        for offset in 0..=src.text.len() {
            let (line, col) = src.position(offset);
            assert_eq!(src.offset(line, col), offset, "offset {offset}");
        }
        assert_eq!(src.position(0), (1, 1));
        assert_eq!(src.position(2), (1, 3));
        assert_eq!(src.position(3), (2, 1));
        assert_eq!(src.position(4), (3, 1));
        assert_eq!(src.position(8), (4, 1));
        assert_eq!(src.offset(9, 9), src.text.len());
    }

    #[test]
    fn blank_and_indentation() {
        let src = source("  \t x\n   \n\ny");
        assert_eq!(src.indentation(1), "  \t ");
        assert!(!src.is_blank(1));
        assert!(src.is_blank(2));
        assert!(src.is_blank(3));
        assert_eq!(src.indentation(4), "");
    }

    #[test]
    fn code_tokens_skip_trivia() {
        let src = source("a = 1 -- c\n;");
        let texts: Vec<&str> = src.code_tokens().map(|token| src.text_of(token)).collect();
        assert_eq!(texts, vec!["a", "=", "1", "-- c", ";", ""]);
        assert_eq!(src.code_token_indexes().len(), 6);
        assert_eq!(src.prev_code(1).map(|token| src.text_of(token)), Some("a"));
        assert_eq!(src.next_code(1).map(|token| src.text_of(token)), Some("1"));
        assert!(src.prev_code(0).is_none());
        assert!(src.next_code(5).is_none());
    }

    #[test]
    fn token_lookup_by_offset() {
        let src = source("ab  'x'");
        assert_eq!(
            src.token_at(0).map(|token| token.kind),
            Some(TokenKind::Name)
        );
        assert_eq!(
            src.token_at(1).map(|token| token.kind),
            Some(TokenKind::Name)
        );
        assert_eq!(
            src.token_at(2).map(|token| token.kind),
            Some(TokenKind::Space)
        );
        assert_eq!(
            src.token_at(5).map(|token| token.kind),
            Some(TokenKind::String { long: false })
        );
        assert!(src.token_at(7).is_none());
        assert!(src.in_string_or_comment(5));
        assert!(!src.in_string_or_comment(0));
    }

    #[test]
    fn tokens_by_line() {
        let src = source("a = [[x\ny]] b\n  c -- d\n");
        let first: Vec<&str> = src
            .code_tokens_on_line(1)
            .map(|token| src.text_of(token))
            .collect();
        assert_eq!(first, vec!["a", "=", "[[x\ny]]"]);
        let second: Vec<&str> = src
            .code_tokens_on_line(2)
            .map(|token| src.text_of(token))
            .collect();
        assert_eq!(second, vec!["b"]);
        assert_eq!(src.tokens_on_line(3).len(), 5);
        assert_eq!(
            src.first_code_token_on_line(3)
                .map(|token| src.text_of(token)),
            Some("c")
        );
        assert_eq!(
            src.last_code_token_on_line(3)
                .map(|token| src.text_of(token)),
            Some("-- d")
        );
        assert_eq!(
            src.last_code_token_on_line(1)
                .map(|token| src.text_of(token)),
            Some("=")
        );
        assert_eq!(
            src.last_code_token_on_line(2)
                .map(|token| src.text_of(token)),
            Some("b")
        );
        assert!(src.first_code_token_on_line(4).is_none());
        assert!(src.last_code_token_on_line(4).is_none());
    }

    #[test]
    fn multiline_token_lines() {
        let src = source("a = [[\n1\n2]]\n--[[\nx\n]]\n'single'\n");
        let mut lines: Vec<u32> = src.lines_inside_multiline_tokens().into_iter().collect();
        lines.sort_unstable();
        assert_eq!(lines, vec![2, 3, 5, 6]);
    }
}
