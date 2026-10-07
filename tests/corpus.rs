//! Autocorrects the whole Flux corpus with every reader at once and checks that the code is
//! unchanged apart from the rewrites the style readers are meant to make.

use std::path::Path;

use proofreader::config::Config;
use proofreader::runner::{Options, ReaderFilter, inspect_source};
use proofreader::source::Source;
use proofreader::token::TokenKind;

/// Where the Flux corpus is checked out.
const CORPUS: &str = "/home/luna/code/flux-ce";

/// A code token reduced to what autocorrection must preserve.
type Normalized = (TokenKind, String);

/// The code tokens of `text`, with the spellings the style readers rewrite mapped onto one form:
/// `not`/`!`, `~=`/`!=`, `&&`/`and` and `||`/`or` are unified, strings are compared by their
/// content with quote escapes dropped, and comments without whitespace.
fn normalized(text: &str) -> Vec<Normalized> {
    let source = Source::new("t.lua", text);
    source
        .code_tokens()
        .filter(|token| token.kind != TokenKind::Eof)
        .map(|token| {
            let text = source.text_of(token);
            match token.kind {
                TokenKind::Not => (TokenKind::Bang, "!".to_owned()),
                TokenKind::Ne => (TokenKind::Ne, "!=".to_owned()),
                TokenKind::AndAnd => (TokenKind::And, "and".to_owned()),
                TokenKind::OrOr => (TokenKind::Or, "or".to_owned()),
                TokenKind::String { long: false } => {
                    let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or(text);
                    (token.kind, without_quote_escapes(inner))
                }
                TokenKind::Comment { .. } => (
                    token.kind,
                    text.chars().filter(|c| !c.is_whitespace()).collect(),
                ),
                _ => (token.kind, text.to_owned()),
            }
        })
        .collect()
}

/// The body of a short string with `\'` and `\"` replaced by the bare quote; every other escape
/// sequence is kept as written.
fn without_quote_escapes(body: &str) -> String {
    let mut result = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        match chars.next() {
            Some(quote @ ('\'' | '"')) => result.push(quote),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}

/// Whether a token may be removed by a correction: `;`, the parentheses around a condition, a
/// trailing comma in a table, or a byte order mark.
fn removable(token: &Normalized) -> bool {
    matches!(
        token.0,
        TokenKind::Semicolon | TokenKind::LParen | TokenKind::RParen | TokenKind::Comma
    ) || (token.0 == TokenKind::Unknown && token.1 == "\u{feff}")
}

/// Whether `after` is `before` with only removable tokens left out.
///
/// Matching greedily is enough: whenever an equal token could be either matched or skipped, both
/// choices are interchangeable because equal tokens are equally removable.
fn only_removals(before: &[Normalized], after: &[Normalized]) -> Result<(), String> {
    let mut kept = after.iter().peekable();
    for (index, token) in before.iter().enumerate() {
        if kept.peek() == Some(&token) {
            kept.next();
        } else if !removable(token) {
            return Err(format!("token {index} {token:?} was changed or removed"));
        }
    }
    match kept.next() {
        Some(extra) => Err(format!("token {extra:?} was added")),
        None => Ok(()),
    }
}

#[test]
#[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
fn all_corrections_keep_the_code_of_the_flux_corpus() {
    let root = Path::new(CORPUS);
    let config = Config::defaults(root);
    let options = Options {
        fix: true,
        filter: ReaderFilter::default(),
        ..Options::default()
    };
    let syntax = Options {
        filter: ReaderFilter::single("Lint/Syntax"),
        ..Options::default()
    };
    let mut changed = 0;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lua"))
    {
        let path = entry.path();
        let text = std::fs::read_to_string(path).expect("UTF-8 source");
        let (_, corrected) = inspect_source(&Source::new(path, text.as_str()), &config, &options);
        let Some(corrected) = corrected else {
            continue;
        };
        changed += 1;
        if let Err(problem) = only_removals(&normalized(&text), &normalized(&corrected)) {
            panic!("{}: {problem}", path.display());
        }
        let (offenses, again) =
            inspect_source(&Source::new(path, corrected.as_str()), &config, &options);
        assert!(again.is_none(), "{} is not idempotent", path.display());
        assert!(
            offenses.iter().all(|offense| !offense.correctable()),
            "{} keeps correctable offenses",
            path.display()
        );
        let (before, _) = inspect_source(&Source::new(path, text.as_str()), &config, &syntax);
        let (after, _) = inspect_source(&Source::new(path, corrected.as_str()), &config, &syntax);
        assert!(
            after.len() <= before.len(),
            "{} has new syntax offenses",
            path.display()
        );
    }
    assert!(changed > 0);
}

#[test]
fn removals_are_checked() {
    let before = normalized("if (not x) then a = { 1, }; end\n");
    assert_eq!(
        only_removals(&before, &normalized("if !x then a = { 1 } end\n")),
        Ok(())
    );
    assert!(only_removals(&before, &normalized("if !y then a = { 1 } end\n")).is_err());
    assert!(
        only_removals(
            &before,
            &normalized("if (not x) then a = { 1, }; end end\n")
        )
        .is_err()
    );
    assert_eq!(
        normalized("x = \"a'b\\\"\" -- c\n"),
        normalized("x = 'a\\'b\"' --c\n")
    );
    assert_eq!(
        normalized("x = \"\\\\\\\"\"\n"),
        normalized("x = '\\\\\"'\n")
    );
    assert_ne!(normalized("x = '\\\\'\n"), normalized("x = '\\''\n"));
}
