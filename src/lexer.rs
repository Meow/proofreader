//! A byte-based GLua lexer that never fails and keeps every byte of the input.
//!
//! Besides standard Lua 5.1 / LuaJIT syntax it understands the GLua extensions `!`, `!=`, `&&`,
//! `||`, `continue`, `//` line comments and `/* */` block comments. Whitespace is emitted as
//! trivia tokens, so concatenating the text of all tokens reproduces the input exactly.

use crate::token::{Token, TokenKind};

/// The UTF-8 encoding of U+FEFF, the byte order mark.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Splits `src` into tokens covering every byte, followed by an empty `Eof` token.
///
/// Unterminated strings and comments end at the end of the input (short strings end at the
/// unescaped line break instead), so lexing never fails.
pub fn lex(src: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        bytes: src.as_bytes(),
        pos: 0,
        line: 1,
        line_start: 0,
        tokens: Vec::with_capacity(src.len() / 3 + 1),
    };
    lexer.run();
    lexer.tokens
}

/// Lexer state: the input, the current offset and the position bookkeeping.
struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    line_start: usize,
    tokens: Vec<Token>,
}

/// Whether `byte` can start an identifier.
fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

/// Whether `byte` can continue an identifier.
fn is_name_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

impl Lexer<'_> {
    /// Returns the byte `n` positions ahead of the cursor.
    fn at(&self, n: usize) -> Option<u8> {
        self.bytes.get(self.pos + n).copied()
    }

    /// Lexes the whole input.
    fn run(&mut self) {
        if self.bytes.starts_with(BOM) {
            self.pos = BOM.len();
            self.emit(TokenKind::Unknown, 0);
        }
        while let Some(byte) = self.at(0) {
            let start = self.pos;
            let kind = self.scan(byte);
            if self.pos == start {
                self.pos += 1;
            }
            self.emit(kind, start);
        }
        self.emit(TokenKind::Eof, self.pos);
    }

    /// Pushes a token spanning `start..self.pos` and advances the line bookkeeping.
    fn emit(&mut self, kind: TokenKind, start: usize) {
        let end = self.pos;
        let line = self.line;
        let col = u32::try_from(start - self.line_start + 1).unwrap_or(u32::MAX);
        let mut end_line = line;
        for (offset, &byte) in self.bytes[start..end].iter().enumerate() {
            if byte == b'\n' {
                let index = start + offset;
                self.line += 1;
                self.line_start = index + 1;
                if index + 1 < end {
                    end_line += 1;
                }
            }
        }
        self.tokens.push(Token {
            kind,
            start,
            end,
            line,
            col,
            end_line,
        });
    }

    /// Scans one token starting with `byte` and returns its kind.
    fn scan(&mut self, byte: u8) -> TokenKind {
        match byte {
            b'\n' => self.single(TokenKind::Newline),
            b'\r' if self.at(1) == Some(b'\n') => {
                self.pos += 2;
                TokenKind::Newline
            }
            b' ' | b'\r' | 0x0b | 0x0c => self.spaces(),
            b'\t' => {
                self.skip_while(|byte| byte == b'\t');
                TokenKind::Tab
            }
            b'-' if self.at(1) == Some(b'-') => self.dash_comment(),
            b'/' if self.at(1) == Some(b'/') => self.line_comment(),
            b'/' if self.at(1) == Some(b'*') => self.block_comment(),
            b'"' | b'\'' => self.short_string(byte),
            b'[' => match self.long_bracket_level() {
                Some(level) => {
                    self.long_bracket(level);
                    TokenKind::String { long: true }
                }
                None => self.single(TokenKind::LBracket),
            },
            b'0'..=b'9' => self.number(),
            b'.' => match (self.at(1), self.at(2)) {
                (Some(b'.'), Some(b'.')) => self.multi(3, TokenKind::Dots),
                (Some(b'.'), _) => self.multi(2, TokenKind::Concat),
                (Some(next), _) if next.is_ascii_digit() => self.number(),
                _ => self.single(TokenKind::Dot),
            },
            b'=' => self.with_eq(TokenKind::Eq, TokenKind::Assign),
            b'~' => self.with_eq(TokenKind::Ne, TokenKind::Tilde),
            b'!' => self.with_eq(TokenKind::Ne, TokenKind::Bang),
            b'<' => self.with_eq(TokenKind::Le, TokenKind::Lt),
            b'>' => self.with_eq(TokenKind::Ge, TokenKind::Gt),
            b'&' if self.at(1) == Some(b'&') => self.multi(2, TokenKind::AndAnd),
            b'|' if self.at(1) == Some(b'|') => self.multi(2, TokenKind::OrOr),
            b'+' => self.single(TokenKind::Plus),
            b'-' => self.single(TokenKind::Minus),
            b'*' => self.single(TokenKind::Star),
            b'/' => self.single(TokenKind::Slash),
            b'%' => self.single(TokenKind::Percent),
            b'^' => self.single(TokenKind::Caret),
            b'#' => self.single(TokenKind::Hash),
            b'(' => self.single(TokenKind::LParen),
            b')' => self.single(TokenKind::RParen),
            b'{' => self.single(TokenKind::LBrace),
            b'}' => self.single(TokenKind::RBrace),
            b']' => self.single(TokenKind::RBracket),
            b';' => self.single(TokenKind::Semicolon),
            b':' => self.single(TokenKind::Colon),
            b',' => self.single(TokenKind::Comma),
            byte if is_name_start(byte) => self.name(),
            _ => self.single(TokenKind::Unknown),
        }
    }

    /// Consumes one byte and returns `kind`.
    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.multi(1, kind)
    }

    /// Consumes `len` bytes and returns `kind`.
    fn multi(&mut self, len: usize, kind: TokenKind) -> TokenKind {
        self.pos += len;
        kind
    }

    /// Returns `with` for an operator followed by `=` (consuming both), otherwise `without`.
    fn with_eq(&mut self, with: TokenKind, without: TokenKind) -> TokenKind {
        if self.at(1) == Some(b'=') {
            self.multi(2, with)
        } else {
            self.single(without)
        }
    }

    /// Advances while `predicate` holds for the current byte.
    fn skip_while(&mut self, predicate: impl Fn(u8) -> bool) {
        while self.at(0).is_some_and(&predicate) {
            self.pos += 1;
        }
    }

    /// Consumes a run of horizontal whitespace other than tabs; a `\r` directly before `\n`
    /// is left for the newline token.
    fn spaces(&mut self) -> TokenKind {
        while let Some(byte) = self.at(0) {
            match byte {
                b' ' | 0x0b | 0x0c => self.pos += 1,
                b'\r' if self.at(1) != Some(b'\n') => self.pos += 1,
                _ => break,
            }
        }
        TokenKind::Space
    }

    /// Consumes an identifier or keyword.
    fn name(&mut self) -> TokenKind {
        let start = self.pos;
        self.skip_while(is_name_continue);
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()
            .and_then(TokenKind::keyword)
            .unwrap_or(TokenKind::Name)
    }

    /// Consumes a number the way LuaJIT does: identifier characters, dots and exponent signs.
    fn number(&mut self) -> TokenKind {
        let exponent = if self.at(0) == Some(b'0') && matches!(self.at(1), Some(b'x' | b'X')) {
            b'p'
        } else {
            b'e'
        };
        let mut previous = 0u8;
        while let Some(byte) = self.at(0) {
            let accepted = byte.is_ascii_alphanumeric()
                || byte == b'_'
                || byte == b'.'
                || (matches!(byte, b'+' | b'-') && previous | 0x20 == exponent);
            if !accepted {
                break;
            }
            previous = byte;
            self.pos += 1;
        }
        TokenKind::Number
    }

    /// Consumes a quoted string; an unescaped line break or the end of input terminates it.
    fn short_string(&mut self, quote: u8) -> TokenKind {
        self.pos += 1;
        while let Some(byte) = self.at(0) {
            match byte {
                b'\\' => {
                    self.pos += 1;
                    match self.at(0) {
                        Some(b'\r') => {
                            self.pos += 1;
                            if self.at(0) == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        Some(b'z') => {
                            self.pos += 1;
                            self.skip_while(|byte| byte.is_ascii_whitespace() || byte == 0x0b);
                        }
                        Some(_) => self.pos += 1,
                        None => {}
                    }
                }
                b'\n' => break,
                b'\r' if self.at(1) == Some(b'\n') => break,
                byte if byte == quote => {
                    self.pos += 1;
                    break;
                }
                _ => self.pos += 1,
            }
        }
        TokenKind::String { long: false }
    }

    /// Returns the level of a long bracket opening at the cursor (`[[` is 0, `[=[` is 1).
    fn long_bracket_level(&self) -> Option<usize> {
        let mut level = 0;
        while self.at(1 + level) == Some(b'=') {
            level += 1;
        }
        (self.at(1 + level) == Some(b'[')).then_some(level)
    }

    /// Consumes a long bracket of `level` starting at the cursor, up to its closing bracket or
    /// the end of input.
    fn long_bracket(&mut self, level: usize) {
        self.pos += level + 2;
        while let Some(byte) = self.at(0) {
            if byte == b']' {
                let closing = (1..=level).all(|n| self.at(n) == Some(b'='));
                if closing && self.at(level + 1) == Some(b']') {
                    self.pos += level + 2;
                    return;
                }
            }
            self.pos += 1;
        }
    }

    /// Consumes a `--` comment, long or not.
    fn dash_comment(&mut self) -> TokenKind {
        self.pos += 2;
        if self.at(0) == Some(b'[')
            && let Some(level) = self.long_bracket_level()
        {
            self.long_bracket(level);
            return TokenKind::Comment { long: true };
        }
        self.line_comment()
    }

    /// Consumes the rest of the line, leaving the line break for the newline token.
    fn line_comment(&mut self) -> TokenKind {
        while let Some(byte) = self.at(0) {
            if byte == b'\n' || (byte == b'\r' && self.at(1) == Some(b'\n')) {
                break;
            }
            self.pos += 1;
        }
        TokenKind::Comment { long: false }
    }

    /// Consumes a `/* */` comment up to its terminator or the end of input.
    fn block_comment(&mut self) -> TokenKind {
        self.pos += 2;
        while let Some(byte) = self.at(0) {
            if byte == b'*' && self.at(1) == Some(b'/') {
                self.pos += 2;
                break;
            }
            self.pos += 1;
        }
        TokenKind::Comment { long: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn check_invariants(src: &str) -> Vec<Token> {
        let tokens = lex(src);
        let last = tokens.last().expect("at least the Eof token");
        assert_eq!(last.kind, Eof);
        assert_eq!((last.start, last.end), (src.len(), src.len()));
        let mut offset = 0;
        let mut line = 1u32;
        let mut line_start = 0usize;
        for token in &tokens {
            assert_eq!(token.start, offset, "tokens must be contiguous in {src:?}");
            assert!(token.end >= token.start);
            assert!(
                token.kind == Eof || token.end > token.start,
                "empty token in {src:?}"
            );
            assert!(src.is_char_boundary(token.start) && src.is_char_boundary(token.end));
            assert_eq!(token.line, line, "line of {token:?} in {src:?}");
            assert_eq!(token.col as usize, token.start - line_start + 1);
            let text = token.text(src);
            let inner_breaks = text
                .char_indices()
                .filter(|&(index, c)| c == '\n' && index + 1 < text.len())
                .count();
            assert_eq!(token.end_line, token.line + inner_breaks as u32);
            for (index, byte) in text.bytes().enumerate() {
                if byte == b'\n' {
                    line += 1;
                    line_start = token.start + index + 1;
                }
            }
            offset = token.end;
        }
        let rebuilt: std::string::String = tokens.iter().map(|token| token.text(src)).collect();
        assert_eq!(rebuilt, src);
        tokens
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        check_invariants(src)
            .into_iter()
            .map(|token| token.kind)
            .filter(|&kind| kind != Eof)
            .collect()
    }

    fn code(src: &str) -> Vec<(TokenKind, &str)> {
        check_invariants(src)
            .into_iter()
            .filter(|token| !token.is_trivia() && token.kind != Eof)
            .map(|token| (token.kind, token.text(src)))
            .collect()
    }

    #[test]
    fn empty_input_is_just_eof() {
        let tokens = check_invariants("");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, Eof);
        assert_eq!(
            (tokens[0].line, tokens[0].col, tokens[0].end_line),
            (1, 1, 1)
        );
    }

    #[test]
    fn simple_statement() {
        assert_eq!(
            kinds("local a = 1"),
            vec![Local, Space, Name, Space, Assign, Space, Number]
        );
    }

    #[test]
    fn keywords() {
        let src = "and break continue do else elseif end false for function goto if in local \
                   nil not or repeat return then true until while";
        let words: Vec<TokenKind> = code(src).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(
            words,
            vec![
                And, Break, Continue, Do, Else, ElseIf, End, False, For, Function, Goto, If, In,
                Local, Nil, Not, Or, Repeat, Return, Then, True, Until, While
            ]
        );
    }

    #[test]
    fn keyword_prefixes_are_names() {
        assert_eq!(
            code("ends endx _end iff nothing continue_"),
            vec![
                (Name, "ends"),
                (Name, "endx"),
                (Name, "_end"),
                (Name, "iff"),
                (Name, "nothing"),
                (Name, "continue_")
            ]
        );
    }

    #[test]
    fn operators() {
        let src = "+ - * / % ^ # == != ~= < <= > >= = ( ) { } [ ] ; : , . .. ... ! ~ && ||";
        let found: Vec<TokenKind> = code(src).into_iter().map(|(kind, _)| kind).collect();
        assert_eq!(
            found,
            vec![
                Plus, Minus, Star, Slash, Percent, Caret, Hash, Eq, Ne, Ne, Lt, Le, Gt, Ge, Assign,
                LParen, RParen, LBrace, RBrace, LBracket, RBracket, Semicolon, Colon, Comma, Dot,
                Concat, Dots, Bang, Tilde, AndAnd, OrOr
            ]
        );
    }

    #[test]
    fn operators_without_spaces() {
        assert_eq!(
            code("a!=b&&!c||d~=e"),
            vec![
                (Name, "a"),
                (Ne, "!="),
                (Name, "b"),
                (AndAnd, "&&"),
                (Bang, "!"),
                (Name, "c"),
                (OrOr, "||"),
                (Name, "d"),
                (Ne, "~="),
                (Name, "e")
            ]
        );
        assert_eq!(
            code("'a'..b..'c'"),
            vec![
                (String { long: false }, "'a'"),
                (Concat, ".."),
                (Name, "b"),
                (Concat, ".."),
                (String { long: false }, "'c'")
            ]
        );
    }

    #[test]
    fn lone_ampersand_and_pipe_are_unknown() {
        assert_eq!(kinds("&|"), vec![Unknown, Unknown]);
        assert_eq!(kinds("@$`\\"), vec![Unknown, Unknown, Unknown, Unknown]);
    }

    #[test]
    fn numbers() {
        for number in [
            "12",
            "0xFF",
            "0XaB",
            "1.5",
            ".5",
            "1e10",
            "1.5e-3",
            "1E+5",
            "0x1p-4",
            "3.",
            "1ULL",
            "0x7fffffffLL",
            "12i",
        ] {
            assert_eq!(code(number), vec![(Number, number)], "{number}");
        }
    }

    #[test]
    fn number_edges() {
        assert_eq!(
            code("a.b.5"),
            vec![(Name, "a"), (Dot, "."), (Name, "b"), (Number, ".5")]
        );
        assert_eq!(
            code("1-2"),
            vec![(Number, "1"), (Minus, "-"), (Number, "2")]
        );
        assert_eq!(
            code("0xE-1"),
            vec![(Number, "0xE"), (Minus, "-"), (Number, "1")]
        );
        assert_eq!(
            code("x..5"),
            vec![(Name, "x"), (Concat, ".."), (Number, "5")]
        );
        assert_eq!(code("1e-"), vec![(Number, "1e-")]);
    }

    #[test]
    fn short_strings() {
        assert_eq!(code("'a'"), vec![(String { long: false }, "'a'")]);
        assert_eq!(
            code(r#""a\"b""#),
            vec![(String { long: false }, r#""a\"b""#)]
        );
        assert_eq!(code(r"'it\'s'"), vec![(String { long: false }, r"'it\'s'")]);
        assert_eq!(code(r#""a'b""#), vec![(String { long: false }, r#""a'b""#)]);
        assert_eq!(code(r"'\\'"), vec![(String { long: false }, r"'\\'")]);
        assert_eq!(
            code(r#""--not a comment""#),
            vec![(String { long: false }, r#""--not a comment""#)]
        );
    }

    #[test]
    fn backslash_newline_continues_a_string() {
        let src = "x = 'a\\\nb' y";
        let tokens = check_invariants(src);
        let string = tokens
            .iter()
            .find(|token| token.kind == String { long: false })
            .expect("a string");
        assert_eq!(string.text(src), "'a\\\nb'");
        assert_eq!((string.line, string.end_line), (1, 2));
        let y = tokens
            .iter()
            .find(|token| token.text(src) == "y")
            .expect("y");
        assert_eq!((y.line, y.col), (2, 4));
    }

    #[test]
    fn backslash_crlf_continues_a_string() {
        assert_eq!(
            code("'a\\\r\nb'"),
            vec![(String { long: false }, "'a\\\r\nb'")]
        );
    }

    #[test]
    fn z_escape_skips_whitespace() {
        assert_eq!(
            code("'a\\z\n   b' c"),
            vec![(String { long: false }, "'a\\z\n   b'"), (Name, "c")]
        );
    }

    #[test]
    fn unterminated_short_string_ends_at_line_break() {
        assert_eq!(
            kinds("'abc\nx"),
            vec![String { long: false }, Newline, Name]
        );
        assert_eq!(
            kinds("'abc\r\nx"),
            vec![String { long: false }, Newline, Name]
        );
        assert_eq!(kinds("\"abc"), vec![String { long: false }]);
        assert_eq!(kinds("'abc\\"), vec![String { long: false }]);
    }

    #[test]
    fn long_strings() {
        assert_eq!(code("[[a]]"), vec![(String { long: true }, "[[a]]")]);
        assert_eq!(
            code("[==[a]]b]=]c]==]"),
            vec![(String { long: true }, "[==[a]]b]=]c]==]")]
        );
        assert_eq!(code("[=[]=]"), vec![(String { long: true }, "[=[]=]")]);
        assert_eq!(
            code("txt[[x]]"),
            vec![(Name, "txt"), (String { long: true }, "[[x]]")]
        );
        assert_eq!(
            code("[[unterminated"),
            vec![(String { long: true }, "[[unterminated")]
        );
        assert_eq!(code("[==[a]=]"), vec![(String { long: true }, "[==[a]=]")]);
    }

    #[test]
    fn long_string_lines() {
        let src = "local s = [[\nfoo\nbar]] x\n";
        let tokens = check_invariants(src);
        let string = tokens
            .iter()
            .find(|token| token.kind == String { long: true })
            .expect("a long string");
        assert_eq!((string.line, string.col, string.end_line), (1, 11, 3));
        let x = tokens
            .iter()
            .find(|token| token.text(src) == "x")
            .expect("x");
        assert_eq!((x.line, x.col), (3, 7));
    }

    #[test]
    fn brackets_that_are_not_long_strings() {
        assert_eq!(
            code("t[i]"),
            vec![(Name, "t"), (LBracket, "["), (Name, "i"), (RBracket, "]")]
        );
        assert_eq!(
            code("t[=]"),
            vec![(Name, "t"), (LBracket, "["), (Assign, "="), (RBracket, "]")]
        );
        assert_eq!(
            code("t[ [[k]] ]"),
            vec![
                (Name, "t"),
                (LBracket, "["),
                (String { long: true }, "[[k]]"),
                (RBracket, "]")
            ]
        );
        assert_eq!(
            code("str[i]"),
            vec![(Name, "str"), (LBracket, "["), (Name, "i"), (RBracket, "]")]
        );
    }

    #[test]
    fn line_comments() {
        assert_eq!(code("-- hi"), vec![(Comment { long: false }, "-- hi")]);
        assert_eq!(code("--- doc"), vec![(Comment { long: false }, "--- doc")]);
        assert_eq!(code("// glua"), vec![(Comment { long: false }, "// glua")]);
        assert_eq!(code("--"), vec![(Comment { long: false }, "--")]);
        assert_eq!(
            code("--[ not long"),
            vec![(Comment { long: false }, "--[ not long")]
        );
        assert_eq!(
            code("--[= not long"),
            vec![(Comment { long: false }, "--[= not long")]
        );
        assert_eq!(
            kinds("a -- c\nb"),
            vec![Name, Space, Comment { long: false }, Newline, Name]
        );
    }

    #[test]
    fn line_comment_leaves_crlf_to_newline() {
        let src = "-- c\r\nx";
        let tokens = check_invariants(src);
        assert_eq!(tokens[0].text(src), "-- c");
        assert_eq!(tokens[1].kind, Newline);
        assert_eq!(tokens[1].text(src), "\r\n");
        assert_eq!((tokens[2].line, tokens[2].col), (2, 1));
    }

    #[test]
    fn long_comments() {
        assert_eq!(
            code("--[[ a\nb ]]"),
            vec![(Comment { long: true }, "--[[ a\nb ]]")]
        );
        assert_eq!(
            code("--[==[ ]] ]==]"),
            vec![(Comment { long: true }, "--[==[ ]] ]==]")]
        );
        assert_eq!(
            code("/* a\n b */"),
            vec![(Comment { long: true }, "/* a\n b */")]
        );
        assert_eq!(code("/**/"), vec![(Comment { long: true }, "/**/")]);
        assert_eq!(code("/* open"), vec![(Comment { long: true }, "/* open")]);
        assert_eq!(
            code("--[[ open"),
            vec![(Comment { long: true }, "--[[ open")]
        );
        assert_eq!(code("/*/"), vec![(Comment { long: true }, "/*/")]);
    }

    #[test]
    fn long_comment_positions() {
        let src = "a\n--[[\nx\n]] b";
        let tokens = check_invariants(src);
        let comment = tokens
            .iter()
            .find(|token| token.kind.is_comment())
            .expect("a comment");
        assert_eq!((comment.line, comment.col, comment.end_line), (2, 1, 4));
        let b = tokens
            .iter()
            .find(|token| token.text(src) == "b")
            .expect("b");
        assert_eq!((b.line, b.col), (4, 4));
    }

    #[test]
    fn slash_is_division_unless_comment() {
        assert_eq!(code("a / b"), vec![(Name, "a"), (Slash, "/"), (Name, "b")]);
        assert_eq!(
            code("a/b//c"),
            vec![
                (Name, "a"),
                (Slash, "/"),
                (Name, "b"),
                (Comment { long: false }, "//c")
            ]
        );
    }

    #[test]
    fn whitespace_tokens() {
        assert_eq!(kinds("  \t\t  x"), vec![Space, Tab, Space, Name]);
        assert_eq!(kinds("\n\n"), vec![Newline, Newline]);
        assert_eq!(kinds("a \r\nb"), vec![Name, Space, Newline, Name]);
        assert_eq!(kinds("a\rb"), vec![Name, Space, Name]);
        assert_eq!(kinds("\x0c\x0b"), vec![Space]);
    }

    #[test]
    fn newline_positions() {
        let tokens = check_invariants("a\nb\r\n\nc");
        let lines: Vec<(u32, u32)> = tokens.iter().map(|token| (token.line, token.col)).collect();
        assert_eq!(
            lines,
            vec![(1, 1), (1, 2), (2, 1), (2, 2), (3, 1), (4, 1), (4, 2)]
        );
        assert!(tokens.iter().all(|token| token.end_line == token.line));
    }

    #[test]
    fn eof_after_trailing_newline_is_on_the_last_line() {
        let tokens = check_invariants("a\n");
        let eof = tokens.last().expect("eof");
        assert_eq!((eof.line, eof.col), (2, 1));
    }

    #[test]
    fn byte_order_mark_is_unknown() {
        let src = "\u{FEFF}local a";
        let tokens = check_invariants(src);
        assert_eq!(tokens[0].kind, Unknown);
        assert_eq!(tokens[0].range(), 0..3);
        assert_eq!(tokens[1].kind, Local);
        assert_eq!((tokens[1].line, tokens[1].col), (1, 4));
    }

    #[test]
    fn non_ascii_names() {
        assert_eq!(
            code("local été = 1"),
            vec![
                (Local, "local"),
                (Name, "été"),
                (Assign, "="),
                (Number, "1")
            ]
        );
        assert_eq!(code("'héllo'"), vec![(String { long: false }, "'héllo'")]);
        assert_eq!(code("1é"), vec![(Number, "1"), (Name, "é")]);
    }

    #[test]
    fn labels_and_goto() {
        assert_eq!(
            code("goto x ::x::"),
            vec![
                (Goto, "goto"),
                (Name, "x"),
                (Colon, ":"),
                (Colon, ":"),
                (Name, "x"),
                (Colon, ":"),
                (Colon, ":")
            ]
        );
    }

    #[test]
    fn method_calls_and_varargs() {
        assert_eq!(
            code("obj:m(...)"),
            vec![
                (Name, "obj"),
                (Colon, ":"),
                (Name, "m"),
                (LParen, "("),
                (Dots, "..."),
                (RParen, ")")
            ]
        );
    }

    #[test]
    fn degenerate_inputs_never_panic() {
        let alphabet = [
            "", "-", "[", "]", "=", "'", "\"", "\\", "\n", "\r", "/", "*", ".", "0", "e", "x", "!",
            "~", "&", "|", " ", "\t", "é", "z", "<", "#",
        ];
        for a in alphabet {
            for b in alphabet {
                for c in alphabet {
                    for d in ["", "-", "[", "\n", "'", "*", "/"] {
                        check_invariants(&format!("{a}{b}{c}{d}"));
                    }
                }
            }
        }
    }

    #[test]
    fn pathological_inputs() {
        for src in [
            "--[==[",
            "--[==[ ]=]",
            "[=",
            "[==",
            "'\\",
            "\"\\z",
            "/*",
            "/*/",
            "0x",
            "...",
            "....",
            "\u{FEFF}",
            "\u{FEFF}\u{FEFF}",
            "\r",
            "\r\r\n",
            "\0",
            "a\0b",
        ] {
            check_invariants(src);
        }
    }

    #[test]
    fn realistic_glua() {
        let src = "\
--- Does a thing.
-- @param actor [Player]
function PLUGIN:PlayerSpawn(actor)
  if !IsValid(actor) then return end

  for k, v in ipairs(self.items) do
    if v.id != 'x' && v.y || !v.z then continue end
    /* block */ local str = \"a\"..v[1] // line
  end
end
";
        let tokens = check_invariants(src);
        let continue_token = tokens
            .iter()
            .find(|token| token.kind == Continue)
            .expect("continue");
        assert_eq!((continue_token.line, continue_token.col), (7, 40));
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.kind.is_comment())
                .count(),
            4
        );
    }

    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
    fn lexes_the_flux_corpus() {
        let root = std::path::Path::new("/home/luna/code/flux-ce");
        let mut count = 0;
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "lua")
                && !path.starts_with(root.join(".git"))
            {
                let text = std::fs::read_to_string(path).expect("UTF-8 source");
                let tokens = check_invariants(&text);
                assert!(
                    tokens
                        .iter()
                        .all(|token| token.kind != Unknown || token.start == 0),
                    "unknown token in {}",
                    path.display()
                );
                count += 1;
            }
        }
        assert!(count >= 357, "only {count} files found");
    }
}
