//! Helpers shared by the token-spacing readers: the whitespace between adjacent code tokens on a
//! line and the classification of operators as unary or binary.

use std::ops::Range;

use crate::source::Source;
use crate::token::{Token, TokenKind};

/// The whitespace between two adjacent code tokens on the same line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap<'a> {
    /// The code token before the gap.
    pub before: &'a Token,
    /// The code token after the gap.
    pub after: &'a Token,
}

impl Gap<'_> {
    /// Byte range of the whitespace, empty when the tokens touch.
    pub fn range(&self) -> Range<usize> {
        self.before.end..self.after.start
    }

    /// Whether the tokens touch.
    pub fn is_empty(&self) -> bool {
        self.before.end == self.after.start
    }

    /// Whether the gap is exactly one space.
    pub fn is_single_space(&self, source: &Source) -> bool {
        &source.text[self.range()] == " "
    }
}

/// The gap between the code tokens with code indexes `code_index - 1` and `code_index`, when both
/// exist, neither is `Eof` and no line break separates them.
pub fn gap_before(source: &Source, code_index: usize) -> Option<Gap<'_>> {
    let indexes = source.code_token_indexes();
    let first = *indexes.get(code_index.checked_sub(1)?)?;
    let second = *indexes.get(code_index)?;
    let after = &source.tokens[second];
    if after.kind == TokenKind::Eof
        || source.tokens[first + 1..second]
            .iter()
            .any(|token| token.kind == TokenKind::Newline)
    {
        return None;
    }
    Some(Gap {
        before: &source.tokens[first],
        after,
    })
}

/// The gap between the code tokens with code indexes `code_index` and `code_index + 1`.
pub fn gap_after(source: &Source, code_index: usize) -> Option<Gap<'_>> {
    gap_before(source, code_index + 1)
}

/// The closest code token before code index `code_index` that is not a comment.
pub fn prev_significant(source: &Source, code_index: usize) -> Option<&Token> {
    (0..code_index)
        .rev()
        .filter_map(|index| source.code_token(index))
        .find(|token| !token.kind.is_comment())
}

/// Whether a token of this kind can end an operand, so that a following `-` is binary.
fn ends_operand(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Name
            | TokenKind::Number
            | TokenKind::String { .. }
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Nil
            | TokenKind::Dots
    )
}

/// Whether the code token at `code_index` is a binary operator (or `=`) whose surroundings
/// `Layout/SpaceAroundOperators` checks; `..` included, unary `-` excluded.
pub fn is_binary_operator(source: &Source, code_index: usize) -> bool {
    let Some(token) = source.code_token(code_index) else {
        return false;
    };
    match token.kind {
        TokenKind::Assign
        | TokenKind::Eq
        | TokenKind::Ne
        | TokenKind::Lt
        | TokenKind::Le
        | TokenKind::Gt
        | TokenKind::Ge
        | TokenKind::Plus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::Caret
        | TokenKind::And
        | TokenKind::Or
        | TokenKind::AndAnd
        | TokenKind::OrOr
        | TokenKind::Concat => true,
        TokenKind::Minus => {
            prev_significant(source, code_index).is_some_and(|prev| ends_operand(prev.kind))
        }
        _ => false,
    }
}

/// Whether a token of this kind is a function-call target that `(` may directly follow.
pub fn is_callee_end(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Name | TokenKind::RParen | TokenKind::RBracket | TokenKind::String { .. }
    )
}

/// Which side of a delimiter pair a gap lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Directly after the opening delimiter.
    Open,
    /// Directly before the closing delimiter.
    Close,
    /// Between an opening delimiter and the closing one right after it.
    Empty,
}

/// The gaps on the inside of every `open`/`close` delimiter pair that lie on one line, skipping
/// gaps next to a comment.
pub fn inside_gaps(source: &Source, open: TokenKind, close: TokenKind) -> Vec<(Side, Gap<'_>)> {
    let mut gaps = Vec::new();
    for code_index in 0..source.code_token_indexes().len() {
        let Some(token) = source.code_token(code_index) else {
            continue;
        };
        if token.kind == open {
            if let Some(gap) =
                gap_after(source, code_index).filter(|gap| !gap.after.kind.is_comment())
            {
                let side = if gap.after.kind == close {
                    Side::Empty
                } else {
                    Side::Open
                };
                gaps.push((side, gap));
            }
        } else if token.kind == close
            && let Some(gap) = gap_before(source, code_index)
                .filter(|gap| !gap.before.kind.is_comment() && gap.before.kind != open)
        {
            gaps.push((Side::Close, gap));
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::Config;
    use crate::runner::{Options, ReaderFilter, inspect_source};

    const SPACING_READERS: [&str; 8] = [
        "Layout/SpaceAroundOperators",
        "Layout/SpaceAfterComma",
        "Layout/SpaceBeforeComma",
        "Layout/SpaceInsideBraces",
        "Layout/SpaceInsideParens",
        "Layout/SpaceInsideBrackets",
        "Layout/SpaceBeforeParen",
        "Layout/ExtraSpacing",
    ];

    fn binary_operators(text: &str) -> Vec<String> {
        let source = Source::new("t.lua", text);
        (0..source.code_token_indexes().len())
            .filter(|&index| is_binary_operator(&source, index))
            .filter_map(|index| source.code_token(index))
            .map(|token| source.text_of(token).to_owned())
            .collect()
    }

    #[test]
    fn tells_unary_from_binary_minus() {
        assert_eq!(binary_operators("x = a - -b"), vec!["=", "-"]);
        assert_eq!(binary_operators("f(-1, t[1] - 2)"), vec!["-"]);
        assert_eq!(binary_operators("return -x"), Vec::<String>::new());
        assert_eq!(
            binary_operators("y = #t..'x' and !z"),
            vec!["=", "..", "and"]
        );
        assert_eq!(binary_operators("y = f() --[[c]] - 1"), vec!["=", "-"]);
    }

    #[test]
    fn finds_inside_gaps() {
        let source = Source::new("t.lua", "f( a, ( ) )\ng(--c\n)\n");
        let sides: Vec<(Side, std::ops::Range<usize>)> =
            inside_gaps(&source, TokenKind::LParen, TokenKind::RParen)
                .into_iter()
                .map(|(side, gap)| (side, gap.range()))
                .collect();
        assert_eq!(
            sides,
            vec![
                (Side::Open, 2..3),
                (Side::Empty, 7..8),
                (Side::Close, 9..10)
            ]
        );
    }

    #[test]
    fn gaps_stay_on_one_line() {
        let source = Source::new("t.lua", "a  =\n  b\n");
        let gap = gap_before(&source, 1).expect("gap");
        assert_eq!(gap.range(), 1..3);
        assert!(!gap.is_empty());
        assert!(!gap.is_single_space(&source));
        assert!(gap_before(&source, 2).is_none());
        assert!(gap_before(&source, 0).is_none());
        assert!(gap_after(&source, 2).is_none());
    }

    /// The kinds and texts of the code tokens of `text`.
    fn code_sequence(text: &str) -> Vec<(TokenKind, String)> {
        let source = Source::new("t.lua", text);
        source
            .code_tokens()
            .map(|token| (token.kind, source.text_of(token).to_owned()))
            .collect()
    }

    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
    fn corrections_keep_the_flux_corpus_tokens() {
        let root = Path::new("/home/luna/code/flux-ce");
        let only: Vec<String> = SPACING_READERS
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        let options = Options {
            fix: true,
            filter: ReaderFilter::new(&only, &[]).expect("readers"),
            ..Options::default()
        };
        let configs = [
            "",
            "Layout/SpaceAroundOperators:\n  ConcatStyle: space\n\
             Layout/SpaceInsideBraces:\n  EnforcedStyle: no_space\n\
             Layout/ExtraSpacing:\n  AllowForAlignment: false\n  ForceEqualSignAlignment: true\n",
        ];
        let mut changed = 0;
        for yaml in configs {
            let config = Config::from_yaml_str(yaml, root).expect("config");
            for entry in walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lua"))
            {
                let path = entry.path();
                let text = std::fs::read_to_string(path).expect("UTF-8 source");
                let (_, corrected) =
                    inspect_source(&Source::new(path, text.clone()), &config, &options);
                let Some(corrected) = corrected else {
                    continue;
                };
                changed += 1;
                assert_eq!(
                    code_sequence(&text),
                    code_sequence(&corrected),
                    "{}",
                    path.display()
                );
                let (offenses, again) =
                    inspect_source(&Source::new(path, corrected), &config, &options);
                assert!(again.is_none(), "{} is not idempotent", path.display());
                assert!(
                    offenses.iter().all(|offense| !offense.correctable()),
                    "{} keeps correctable offenses: {:?}",
                    path.display(),
                    offenses
                        .iter()
                        .filter(|offense| offense.correctable())
                        .map(|offense| (
                            offense.reader,
                            offense.line,
                            offense.col,
                            &offense.message
                        ))
                        .collect::<Vec<_>>()
                );
            }
        }
        assert!(changed > 0);
    }
}
