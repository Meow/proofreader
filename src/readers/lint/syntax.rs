//! `Lint/Syntax`.

use crate::offense::Severity;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::nesting::{Frame, Nesting, Step};
use crate::source::Source;
use crate::token::TokenKind;

/// Reports what the lexer and a bracket/block counter can tell about broken syntax:
/// unterminated strings and block comments, characters that are not GLua, and the first
/// unbalanced bracket, `end` or `until`.
pub struct Syntax;

impl Reader for Syntax {
    fn name(&self) -> &'static str {
        "Lint/Syntax"
    }

    fn description(&self) -> &'static str {
        "Checks for unterminated literals, stray characters and unbalanced blocks."
    }

    fn default_severity(&self) -> Severity {
        Severity::Fatal
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let mut nesting = Nesting::default();
        let mut balanced = true;
        let mut last = None;
        for token in source.code_tokens() {
            let text = source.text_of(token);
            match token.kind {
                TokenKind::String { .. } | TokenKind::Comment { .. } => {
                    if let Some(message) = unterminated(token.kind, text) {
                        ctx.add_offense(token.range(), message);
                    }
                    if token.kind.is_string() {
                        last = Some(*token);
                    }
                    continue;
                }
                TokenKind::Unknown if token.start == 0 && text == "\u{feff}" => continue,
                TokenKind::Unknown | TokenKind::Tilde => {
                    ctx.add_offense(token.range(), format!("Unexpected character `{text}`."));
                    continue;
                }
                TokenKind::Eof => {
                    if balanced && let Some(frame) = nesting.top() {
                        let anchor = last.unwrap_or(*token);
                        ctx.add_offense(anchor.range(), missing_closer(source, frame));
                    }
                    continue;
                }
                _ => {}
            }
            last = Some(*token);
            if !balanced {
                continue;
            }
            if let Step::Unexpected(open) = nesting.step(token) {
                balanced = false;
                ctx.add_offense(token.range(), unexpected(source, text, open.as_ref()));
            }
        }
    }
}

/// The message for an unterminated string or comment token, if `text` is one.
fn unterminated(kind: TokenKind, text: &str) -> Option<&'static str> {
    match kind {
        TokenKind::String { long: false } => {
            let quote = text.chars().next()?;
            let body = text.strip_prefix(quote)?;
            let closed = body.strip_suffix(quote).is_some_and(|inner| {
                inner.chars().rev().take_while(|&c| c == '\\').count() % 2 == 0
            });
            (!closed).then_some("Unterminated string.")
        }
        TokenKind::String { long: true } => {
            (!long_bracket_closed(text)).then_some("Unterminated long string.")
        }
        TokenKind::Comment { long: true } => {
            let closed = match text.strip_prefix("--") {
                Some(bracket) => long_bracket_closed(bracket),
                None => text.len() >= 4 && text.ends_with("*/"),
            };
            (!closed).then_some("Unterminated block comment.")
        }
        _ => None,
    }
}

/// Whether a long bracket literal (`[==[ ... ]==]`) ends with its matching closing bracket.
fn long_bracket_closed(text: &str) -> bool {
    let level = text
        .strip_prefix('[')
        .map_or(0, |rest| rest.len() - rest.trim_start_matches('=').len());
    let closing = format!("]{}]", "=".repeat(level));
    text.len() >= 2 * closing.len() && text.ends_with(&closing)
}

/// The message for a closer that does not match the innermost open construct.
fn unexpected(source: &Source, text: &str, open: Option<&Frame>) -> String {
    match open {
        Some(frame) => format!(
            "Unexpected `{text}`; expected `{}` for `{}` on line {}.",
            frame.opener.closer(),
            source.text_of(&frame.token),
            frame.token.line
        ),
        None => format!("Unexpected `{text}`."),
    }
}

/// The message for a construct still open at the end of the file.
fn missing_closer(source: &Source, frame: &Frame) -> String {
    format!(
        "Missing `{}` for `{}` on line {}.",
        frame.opener.closer(),
        source.text_of(&frame.token),
        frame.token.line
    )
}

inventory::submit! { Registration(|| Box::new(Syntax)) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const READER: &str = "Lint/Syntax";

    #[test]
    fn accepts_valid_code() {
        expect_no_offenses(
            READER,
            "local function f(a, ...)\n  for i = 1, 2 do\n    while a do break end\n  end\n\n  \
             repeat a = { [1] = (2) } until a\n\n  if a then elseif b then else end\n\n  \
             do return 'x\\'' end\nend\n",
        );
        expect_no_offenses(
            READER,
            "local s = [==[a]]b]==] --[[ c ]]\n/* d */ x = \"\\\\\"\n",
        );
        expect_no_offenses(READER, "\u{feff}x = 1\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn reports_with_fatal_severity() {
        let offenses = inspect(READER, "x = 'a\n");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].severity, Severity::Fatal);
    }

    #[test]
    fn flags_unterminated_literals() {
        expect_offense(READER, "x = 'abc\ny = 1\n", 1, 5, "Unterminated string.");
        expect_offense(READER, "x = \"abc\\\"\n", 1, 5, "Unterminated string.");
        expect_offense(READER, "x = '\n", 1, 5, "Unterminated string.");
        expect_offense(READER, "x = [[abc\n", 1, 5, "Unterminated long string.");
        expect_offense(READER, "x = [==[abc]]\n", 1, 5, "Unterminated long string.");
        expect_offense(READER, "--[[ abc\n", 1, 1, "Unterminated block comment.");
        expect_offense(READER, "/* abc *\n", 1, 1, "Unterminated block comment.");
    }

    #[test]
    fn flags_unknown_characters() {
        expect_offense(READER, "x = 1 @ 2\n", 1, 7, "Unexpected character `@`.");
        expect_offense(READER, "x = ~y\n", 1, 5, "Unexpected character `~`.");
    }

    #[test]
    fn flags_missing_closers_at_the_last_code_token() {
        expect_offense(
            READER,
            "function f()\n  x()\n",
            2,
            5,
            "Missing `end` for `function` on line 1.",
        );
        expect_offense(READER, "x = { 1\n", 1, 7, "Missing `}` for `{` on line 1.");
        expect_offense(
            READER,
            "repeat\n  x()\n",
            2,
            5,
            "Missing `until` for `repeat` on line 1.",
        );
        expect_offense(
            READER,
            "while x\n",
            1,
            7,
            "Missing `do` for `while` on line 1.",
        );
        expect_offense(
            READER,
            "if x then\n  y = 'a'\n-- trailing\n\n",
            2,
            7,
            "Missing `end` for `if` on line 1.",
        );
    }

    #[test]
    fn flags_the_first_unexpected_closer() {
        expect_offense(READER, "x()\nend\nend\n", 2, 1, "Unexpected `end`.");
        expect_offense(
            READER,
            "if x then\n  f(a]\nend\n",
            2,
            6,
            "Unexpected `]`; expected `)` for `(` on line 2.",
        );
        expect_offense(
            READER,
            "for i = 1, 2\n  x()\nend\n",
            3,
            1,
            "Unexpected `end`; expected `do` for `for` on line 1.",
        );
        expect_offense(READER, "x = 1\nelse\n", 2, 1, "Unexpected `else`.");
        expect_offense(
            READER,
            "while x do\nuntil y\n",
            2,
            1,
            "Unexpected `until`; expected `end` for `while` on line 1.",
        );
    }

    #[test]
    fn ignores_keywords_in_strings_and_comments() {
        expect_no_offenses(READER, "x = 'end' -- end )\ny = [[ ( ]]\n");
    }

    #[test]
    fn never_autocorrects() {
        let src = "x = 'a\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
