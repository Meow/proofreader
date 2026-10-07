//! `Lint/DuplicateTableKey`.

use std::collections::HashSet;
use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::nesting::{Nesting, Opener, Step};
use crate::readers::naming::declarations::significant_tokens;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags a literal key that appears twice in one table constructor: `a = 1, a = 2`,
/// `['a'] = 1, a = 2` or `[1] = x, [1.0] = y`. Nested constructors are checked separately.
pub struct DuplicateTableKey;

/// A table constructor being scanned.
struct Table {
    /// Nesting depth at which the constructor's own fields appear.
    depth: usize,
    /// Normalised keys seen so far.
    keys: HashSet<String>,
    /// Whether the next token starts a field.
    field_start: bool,
}

/// A literal key: its normalised identity, its text for messages and its byte range.
struct Key {
    /// Normalised identity: `s:` plus the string, `n:` plus the number or `b:` plus the boolean.
    identity: String,
    /// The key as shown in messages.
    display: String,
    /// The key's byte range: the name, or the brackets and what they hold.
    range: Range<usize>,
}

impl Reader for DuplicateTableKey {
    fn name(&self) -> &'static str {
        "Lint/DuplicateTableKey"
    }

    fn description(&self) -> &'static str {
        "Checks for duplicate literal keys in table constructors."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let tokens = significant_tokens(source);
        let mut nesting = Nesting::default();
        let mut tables: Vec<Table> = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            if let Some(table) = tables
                .last_mut()
                .filter(|table| table.depth == nesting.frames.len())
            {
                if table.field_start {
                    table.field_start = false;
                    if let Some(key) = field_key(source, &tokens[index..])
                        && !table.keys.insert(key.identity)
                    {
                        ctx.add_offense(
                            key.range,
                            format!("Duplicate key `{}` in table constructor.", key.display),
                        );
                    }
                }
                if matches!(token.kind, TokenKind::Comma | TokenKind::Semicolon) {
                    table.field_start = true;
                }
            }
            match nesting.step(token) {
                Step::Opened if token.kind == TokenKind::LBrace => tables.push(Table {
                    depth: nesting.frames.len(),
                    keys: HashSet::new(),
                    field_start: true,
                }),
                Step::Closed(frame) if frame.opener == Opener::Brace => {
                    tables.pop();
                }
                _ => {}
            }
        }
    }
}

/// The literal key of the field starting at `tokens[0]`, if it has one.
fn field_key(source: &Source, tokens: &[Token]) -> Option<Key> {
    let kinds: Vec<TokenKind> = tokens.iter().take(4).map(|token| token.kind).collect();
    match kinds.as_slice() {
        [TokenKind::Name, TokenKind::Assign, ..] => {
            let name = source.text_of(&tokens[0]);
            Some(Key {
                identity: format!("s:{name}"),
                display: name.to_owned(),
                range: tokens[0].range(),
            })
        }
        [
            TokenKind::LBracket,
            key,
            TokenKind::RBracket,
            TokenKind::Assign,
        ] => {
            let text = source.text_of(&tokens[1]);
            let identity = match key {
                TokenKind::String { long: false } => format!("s:{}", string_content(text)?),
                TokenKind::Number => format!("n:{}", normalise_number(text)),
                TokenKind::True | TokenKind::False => format!("b:{text}"),
                _ => return None,
            };
            let display = match key {
                TokenKind::String { .. } => string_content(text)?.to_owned(),
                _ => text.to_owned(),
            };
            Some(Key {
                identity,
                display,
                range: tokens[0].start..tokens[2].end,
            })
        }
        _ => None,
    }
}

/// The content of a terminated short string literal without escapes.
fn string_content(literal: &str) -> Option<&str> {
    let quote = literal.chars().next()?;
    let inner = literal.strip_prefix(quote)?.strip_suffix(quote)?;
    (!inner.contains('\\')).then_some(inner)
}

/// A canonical spelling of a numeric literal, so that `1`, `1.0` and `0x1` compare equal.
fn normalise_number(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let value = match lower.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok().map(|n| n as f64),
        None => lower.parse::<f64>().ok(),
    };
    value.map_or(lower, |n| n.to_string())
}

inventory::submit! { Registration(|| Box::new(DuplicateTableKey)) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const READER: &str = "Lint/DuplicateTableKey";

    fn message(key: &str) -> String {
        format!("Duplicate key `{key}` in table constructor.")
    }

    #[test]
    fn flags_duplicate_name_keys() {
        expect_offense(READER, "t = { a = 1, a = 2 }\n", 1, 14, &message("a"));
        expect_offense(
            READER,
            "t = {\n  a = 1,\n  b = 2,\n  a = 3\n}\n",
            4,
            3,
            &message("a"),
        );
    }

    #[test]
    fn treats_strings_and_names_alike() {
        expect_offense(
            READER,
            "t = { ['a'] = 1, [\"a\"] = 2 }\n",
            1,
            18,
            &message("a"),
        );
        expect_offense(READER, "t = { a = 1, ['a'] = 2 }\n", 1, 14, &message("a"));
        expect_offense(
            READER,
            "t = { ['a b'] = 1; ['a b'] = 2 }\n",
            1,
            20,
            &message("a b"),
        );
    }

    #[test]
    fn flags_duplicate_numeric_and_boolean_keys() {
        expect_offense(
            READER,
            "t = { [1] = 'x', [1] = 'y' }\n",
            1,
            18,
            &message("1"),
        );
        expect_offense(
            READER,
            "t = { [1] = 'x', [1.0] = 'y' }\n",
            1,
            18,
            &message("1.0"),
        );
        expect_offense(
            READER,
            "t = { [0x10] = 1, [16] = 2 }\n",
            1,
            19,
            &message("16"),
        );
        expect_offense(
            READER,
            "t = { [true] = 1, [true] = 2 }\n",
            1,
            19,
            &message("true"),
        );
    }

    #[test]
    fn reports_every_repetition() {
        expect_offenses(
            READER,
            "t = { a = 1, a = 2, a = 3 }\n",
            &[(1, 14, &message("a")), (1, 21, &message("a"))],
        );
    }

    #[test]
    fn nested_tables_are_separate() {
        expect_no_offenses(READER, "t = { a = { a = 1 }, b = { a = 2 } }\n");
        expect_offense(
            READER,
            "t = { a = { b = 1, b = 2 }, c = 3 }\n",
            1,
            20,
            &message("b"),
        );
        expect_offense(
            READER,
            "t = { x = { y = 1 }, x = 2 }\n",
            1,
            22,
            &message("x"),
        );
    }

    #[test]
    fn ignores_values_and_nested_code() {
        expect_no_offenses(
            READER,
            "t = { a = b, c = b, [b] = 1, [b] = 2, f(a), f(a) }\n",
        );
        expect_no_offenses(
            READER,
            "t = { f = function(x) local a, a = 1, 2 return { x = x } end, g = 1 }\n",
        );
        expect_no_offenses(READER, "t = { a = f(b, a), b = 1 }\nf(a = 1)\n");
        expect_no_offenses(READER, "t = { a = x == y, ['b\\'c'] = 1, [\"b'c\"] = 2 }\n");
        expect_no_offenses(
            READER,
            "t = { [i] = 1, [i] = 2, ['a'..b] = 1, ['a'..b] = 2 }\n",
        );
    }

    #[test]
    fn skips_comments() {
        expect_offense(
            READER,
            "t = {\n  a = 1, -- c\n  --[[ d ]] a = 2\n}\n",
            3,
            13,
            &message("a"),
        );
    }

    #[test]
    fn normalises_numbers() {
        assert_eq!(normalise_number("1"), normalise_number("1.0"));
        assert_eq!(normalise_number("0xFF"), "255");
        assert_eq!(normalise_number("1e2"), "100");
        assert_eq!(normalise_number("0xZZ"), "0xzz");
    }

    #[test]
    fn never_autocorrects() {
        let src = "t = { a = 1, a = 2 }\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
