//! A byte-based GLua lexer that never fails and keeps every byte of the input.
//!
//! Besides standard Lua 5.1 / LuaJIT syntax it understands the GLua extensions `!`, `!=`, `&&`,
//! `||`, `continue`, `//` line comments and `/* */` block comments. Whitespace is emitted as
//! trivia tokens, so concatenating the text of all tokens reproduces the input exactly.
//!
//! [`lex_lumen`] lexes Lumen templates, Lua with markup in expression position, emitting the
//! markup as [`TokenKind::Markup`] tokens and the Lua spliced into it as ordinary tokens (see
//! [`crate::lumen`]). Malformed markup is recorded as a [`MarkupError`] and lexing goes on.

use std::ops::Range;

use crate::lumen::{Markup, MarkupError, Tag, is_component_name};
use crate::token::{Token, TokenKind};

/// The UTF-8 encoding of U+FEFF, the byte order mark.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// How deeply markup elements may nest in a Lumen template; a `<` that would open an element
/// deeper than this is recorded as malformed markup and lexed as Lua or as text, so that lexing
/// never exhausts the stack.
const MAX_MARKUP_DEPTH: usize = 200;

/// Splits `src` into tokens covering every byte, followed by an empty `Eof` token.
///
/// Unterminated strings and comments end at the end of the input (short strings end at the
/// unescaped line break instead), so lexing never fails.
pub fn lex(src: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(src);
    lexer.run(false);
    lexer.tokens
}

/// Splits a Lumen template into tokens like [`lex`], with markup as [`TokenKind::Markup`]
/// tokens, and returns what was found out about its markup.
///
/// Markup that the Lumen compiler would reject is recorded in [`Markup::errors`]; the element
/// it occurs in ends there and lexing resumes in the surrounding markup or Lua, so lexing never
/// fails and every byte is still covered by exactly one token.
pub fn lex_lumen(src: &str) -> (Vec<Token>, Markup) {
    let mut lexer = Lexer::new(src);
    lexer.run(true);
    (lexer.tokens, lexer.markup)
}

/// Lexer state: the input, the current offset and the position bookkeeping.
struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: u32,
    line_start: usize,
    tokens: Vec<Token>,
    /// What was found out about the markup of a Lumen template.
    markup: Markup,
    /// Start of the markup token being built, while lexing markup.
    pending: Option<usize>,
    /// How many markup elements are open.
    depth: usize,
}

/// Whether `byte` can start an identifier.
fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

/// Whether `byte` can continue an identifier.
fn is_name_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// Whether `byte` can start a markup tag or attribute name.
fn is_markup_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Whether `byte` can continue a markup tag or attribute name.
fn is_markup_name_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

/// Whether `byte` is whitespace inside markup (Lua's `%s`).
fn is_markup_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// Whether a token of this kind can end an operand, so that a `<` after it is the less-than
/// operator rather than the start of markup.
fn ends_operand(kind: TokenKind) -> bool {
    kind.ends_operand() || kind == TokenKind::End
}

impl<'a> Lexer<'a> {
    /// A lexer at the start of `src`.
    fn new(src: &'a str) -> Self {
        Lexer {
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            line_start: 0,
            tokens: Vec::with_capacity(src.len() / 3 + 1),
            markup: Markup::default(),
            pending: None,
            depth: 0,
        }
    }

    /// Returns the byte `n` positions ahead of the cursor.
    fn at(&self, n: usize) -> Option<u8> {
        self.bytes.get(self.pos + n).copied()
    }

    /// Lexes the whole input, as a Lumen template with `lumen`.
    fn run(&mut self, lumen: bool) {
        if self.bytes.starts_with(BOM) {
            self.pos = BOM.len();
            self.emit(TokenKind::Unknown, 0);
        }
        if lumen {
            self.lua(false);
        } else {
            while let Some(byte) = self.at(0) {
                self.token(byte);
            }
        }
        self.emit(TokenKind::Eof, self.pos);
    }

    /// Lexes and emits the token starting with `byte` at the cursor and returns its kind.
    fn token(&mut self, byte: u8) -> TokenKind {
        let start = self.pos;
        let kind = self.scan(byte);
        if self.pos == start {
            self.pos += 1;
        }
        self.emit(kind, start);
        kind
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

/// Lexing Lumen templates: Lua mode with markup in expression position, and markup mode.
impl Lexer<'_> {
    /// Whether the bytes at the cursor start with `prefix`.
    fn looking_at(&self, prefix: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(prefix)
    }

    /// Lexes Lua code, turning every `<` that starts markup into an element. With `splice`, stops
    /// at the first `}` not matched by a `{` of its own, which is left for the markup around the
    /// splice. Returns how many tokens other than trivia and comments it lexed, an element
    /// counting as one.
    ///
    /// A name ending in a non-ASCII byte does not end an operand here, as the compiler only takes
    /// ASCII letters, digits and `_` as name characters.
    fn lua(&mut self, splice: bool) -> usize {
        let mut braces = 0usize;
        let mut operand = false;
        let mut count = 0;
        while let Some(byte) = self.at(0) {
            if byte == b'}' && splice && braces == 0 {
                break;
            }
            if byte == b'<' && !operand && self.before_tag() {
                if self.depth < MAX_MARKUP_DEPTH {
                    let start = self.pos;
                    self.element();
                    self.flush_markup();
                    if self.depth == 0 {
                        self.markup.elements.push(start..self.pos);
                    }
                    operand = true;
                    count += 1;
                    continue;
                }
                self.too_deep();
            }
            let kind = self.token(byte);
            match kind {
                TokenKind::LBrace => braces += 1,
                TokenKind::RBrace => braces = braces.saturating_sub(1),
                _ => {}
            }
            if !kind.is_trivia() && !kind.is_comment() {
                operand = ends_operand(kind) && self.bytes[self.pos - 1].is_ascii();
                count += 1;
            }
        }
        count
    }

    /// Whether a name or `>` follows the `<` under the cursor, so that it may open an element.
    fn before_tag(&self) -> bool {
        self.at(1)
            .is_some_and(|next| next == b'>' || is_markup_name_start(next))
    }

    /// Records the `<` under the cursor, which would open an element deeper than
    /// [`MAX_MARKUP_DEPTH`], as malformed markup.
    fn too_deep(&mut self) {
        let here = self.pos;
        self.markup_error(here..here + 1, "markup nested too deeply".to_owned());
    }

    /// Starts a markup token at the cursor unless one is being built.
    fn mark(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.pos);
        }
    }

    /// Emits the markup token being built, if it covers any bytes; whitespace at its end, which
    /// only malformed or unfinished markup leaves there, becomes trivia.
    fn flush_markup(&mut self) {
        let Some(start) = self.pending.take() else {
            return;
        };
        let end = self.pos;
        let content = start
            + self.bytes[start..end]
                .iter()
                .rposition(|&byte| !is_markup_space(byte))
                .map_or(0, |last| last + 1);
        if content > start {
            self.pos = content;
            self.emit(TokenKind::Markup, start);
        }
        self.trivia_until(end);
    }

    /// Lexes the whitespace from the cursor up to byte `end` as trivia tokens.
    fn trivia_until(&mut self, end: usize) {
        while self.pos < end {
            self.token(self.bytes[self.pos]);
        }
    }

    /// Records malformed markup at `range`.
    fn markup_error(&mut self, range: Range<usize>, message: String) {
        self.markup.errors.push(MarkupError { range, message });
    }

    /// The range of the character at byte offset `at`, empty at the end of the input.
    fn char_at(&self, at: usize) -> Range<usize> {
        let width = match self.bytes.get(at) {
            None => 0,
            Some(&byte) if byte < 0xC0 => 1,
            Some(&byte) if byte < 0xE0 => 2,
            Some(&byte) if byte < 0xF0 => 3,
            Some(_) => 4,
        };
        at..(at + width).min(self.bytes.len())
    }

    /// The text of `range`, which lies on character boundaries of the input.
    fn text(&self, range: Range<usize>) -> String {
        String::from_utf8_lossy(&self.bytes[range]).into_owned()
    }

    /// Consumes whitespace inside markup. Whitespace within a line stays in the markup token;
    /// a run with a line break ends the markup token before it and becomes trivia tokens.
    fn markup_space(&mut self) {
        let start = self.pos;
        let mut end = start;
        let mut newline = false;
        while let Some(&byte) = self.bytes.get(end) {
            if !is_markup_space(byte) {
                break;
            }
            newline |= byte == b'\n';
            end += 1;
        }
        if newline {
            self.flush_markup();
            self.trivia_until(end);
        } else {
            self.mark();
            self.pos = end;
        }
        if self.pos < self.bytes.len() {
            self.mark();
        }
    }

    /// Consumes a tag or attribute name at the cursor and returns its range.
    fn markup_name(&mut self) -> Range<usize> {
        let start = self.pos;
        if self.at(0).is_some_and(is_markup_name_start) {
            self.skip_while(is_markup_name_continue);
        }
        start..self.pos
    }

    /// Lexes the Lua of a splice whose `{` or `{...` was just consumed, then its closing `}`.
    /// Returns how many tokens the expression has, or `None` when the input ends first.
    fn splice(&mut self) -> Option<usize> {
        self.flush_markup();
        let count = self.lua(true);
        self.at(0)?;
        self.markup.splice_ends.push(self.pos);
        self.mark();
        self.pos += 1;
        Some(count)
    }

    /// Lexes one element starting at the `<` under the cursor, leaving the cursor after it.
    fn element(&mut self) {
        let start = self.pos;
        self.depth += 1;
        self.mark();
        self.pos += 1;
        if self.at(0) == Some(b'>') {
            self.pos += 1;
            self.children(start, None);
        } else {
            let name = self.markup_name();
            let text = self.text(name.clone());
            if is_component_name(&text) && text.contains('-') {
                self.markup_error(
                    start..name.end,
                    format!(
                        "the name of the '{text}' component must be a Lua expression and can not \
                         have '-' in it"
                    ),
                );
            }
            self.markup.tags.push(Tag {
                name: name.clone(),
                closing: false,
            });
            if self.attributes(start, &name) == Some(false) {
                self.children(start, Some(name));
            }
        }
        self.depth -= 1;
    }

    /// Lexes the attributes of the tag `name` opened at `start`, up to and including the `>` or
    /// `/>` ending the tag. Returns whether the tag is self-closing, or `None` when the markup is
    /// malformed.
    fn attributes(&mut self, start: usize, name: &Range<usize>) -> Option<bool> {
        let tag = self.text(name.clone());
        loop {
            self.markup_space();
            let here = self.pos;
            match self.at(0) {
                None => {
                    self.markup_error(start..name.end, format!("unfinished tag '<{tag}'"));
                    return None;
                }
                Some(b'/') if self.at(1) == Some(b'>') => {
                    self.pos += 2;
                    return Some(true);
                }
                Some(b'/') => {
                    self.markup_error(here..here + 1, "'>' expected after '/'".to_owned());
                    return None;
                }
                Some(b'>') => {
                    self.pos += 1;
                    return Some(false);
                }
                Some(b'{') if self.looking_at(b"{...") => {
                    self.pos += 4;
                    match self.splice() {
                        None => {
                            self.unfinished_expression(here);
                            return None;
                        }
                        Some(0) => self.markup_error(
                            here..here + 4,
                            "spread expression expected after ...".to_owned(),
                        ),
                        Some(_) => {}
                    }
                }
                Some(byte) if is_markup_name_start(byte) => {
                    let attribute = self.markup_name();
                    self.attribute_value(&self.text(attribute))?;
                }
                Some(_) => {
                    self.markup_error(
                        self.char_at(here),
                        format!("attribute name expected in '<{tag}'"),
                    );
                    return None;
                }
            }
        }
    }

    /// Lexes the `= value` after the attribute `name`, if there is one. Returns `None` when the
    /// value is malformed.
    fn attribute_value(&mut self, name: &str) -> Option<()> {
        self.markup_space();
        if self.at(0) != Some(b'=') {
            return Some(());
        }
        self.pos += 1;
        self.markup_space();
        let here = self.pos;
        match self.at(0) {
            Some(quote @ (b'"' | b'\'')) => {
                let Some(length) = self.bytes[here + 1..].iter().position(|&b| b == quote) else {
                    self.markup_error(
                        here..here + 1,
                        format!("unfinished value of the '{name}' attribute"),
                    );
                    return None;
                };
                self.pos = here + length + 2;
            }
            Some(b'{') => {
                self.pos += 1;
                match self.splice() {
                    None => {
                        self.unfinished_expression(here);
                        return None;
                    }
                    Some(0) => self.markup_error(
                        here..here + 1,
                        format!("the '{name}' attribute has an empty expression"),
                    ),
                    Some(_) => {}
                }
            }
            _ => {
                self.markup_error(
                    self.char_at(here),
                    format!("the value of the '{name}' attribute must be quoted or in {{ }}"),
                );
                return None;
            }
        }
        Some(())
    }

    /// Records a splice opened at `start` that the end of the input cuts short.
    fn unfinished_expression(&mut self, start: usize) {
        self.markup_error(
            start..start + 1,
            "unfinished expression: '}' expected".to_owned(),
        );
    }

    /// Lexes the children of the element opened at `start` (the tag `name`, or a fragment), up
    /// to and including its closing tag.
    fn children(&mut self, start: usize, name: Option<Range<usize>>) {
        let tag = name.clone().map(|name| self.text(name));
        loop {
            let here = self.pos;
            match self.at(0) {
                None => {
                    let message = match &tag {
                        Some(tag) => format!("'<{tag}>' is never closed"),
                        None => "the fragment is never closed".to_owned(),
                    };
                    let end = name.as_ref().map_or(start + 2, |name| name.end);
                    self.markup_error(start..end, message);
                    return;
                }
                Some(b'<') if self.looking_at(b"<!--") => {
                    match self.bytes[here + 4..]
                        .windows(3)
                        .position(|window| window == b"-->")
                    {
                        Some(offset) => self.pos = here + 4 + offset + 3,
                        None => {
                            self.markup_error(here..here + 4, "unfinished comment".to_owned());
                            self.pos = self.bytes.len();
                            if let Some(comment) = self.pending.take() {
                                self.emit(TokenKind::Markup, comment);
                            }
                            return;
                        }
                    }
                }
                Some(b'<') if self.at(1) == Some(b'/') => {
                    self.pos += 2;
                    self.closing_tag(tag.as_deref());
                    return;
                }
                Some(b'<') => {
                    if !self.before_tag() {
                        self.markup_error(here..here + 1, "tag name expected".to_owned());
                        self.pos += 1;
                    } else if self.depth < MAX_MARKUP_DEPTH {
                        self.element();
                    } else {
                        self.too_deep();
                        self.pos += 1;
                    }
                }
                Some(b'{') => {
                    self.pos += 1;
                    if self.splice().is_none() {
                        self.unfinished_expression(here);
                        return;
                    }
                }
                Some(_) => {
                    while let Some(byte) = self.at(0) {
                        match byte {
                            b'<' | b'{' => break,
                            byte if is_markup_space(byte) => self.markup_space(),
                            _ => self.pos += 1,
                        }
                    }
                }
            }
        }
    }

    /// Lexes a closing tag after its `</`, which must close `tag` (`None` for a fragment).
    fn closing_tag(&mut self, tag: Option<&str>) {
        self.markup_space();
        if let Some(tag) = tag {
            let name = self.markup_name();
            let found = self.text(name.clone());
            if found != tag {
                let range = if found.is_empty() {
                    self.char_at(name.start)
                } else {
                    name
                };
                let got = self.text(range.clone());
                self.markup_error(range, format!("'</{tag}>' expected, got '</{got}'"));
                return;
            }
            self.markup.tags.push(Tag {
                name,
                closing: true,
            });
            self.markup_space();
        }
        if self.at(0) == Some(b'>') {
            self.pos += 1;
        } else {
            let what = tag.unwrap_or("the fragment");
            self.markup_error(
                self.char_at(self.pos),
                format!("'>' expected in the closing tag of '{what}'"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::Markup;
    use TokenKind::*;

    fn check_invariants(src: &str) -> Vec<Token> {
        check_tokens(src, lex(src))
    }

    fn check_tokens(src: &str, tokens: Vec<Token>) -> Vec<Token> {
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

    fn check_lumen(src: &str) -> (Vec<Token>, crate::lumen::Markup) {
        let (tokens, markup) = lex_lumen(src);
        let tokens = check_tokens(src, tokens);
        for token in tokens.iter().filter(|token| token.kind == Markup) {
            let text = token.text(src);
            assert!(
                !text.starts_with(char::is_whitespace)
                    && (token.end == src.len() || !text.ends_with(char::is_whitespace)),
                "markup token {text:?} has whitespace at an edge in {src:?}"
            );
        }
        for error in &markup.errors {
            assert!(
                error.range.start <= error.range.end
                    && src.is_char_boundary(error.range.start)
                    && src.is_char_boundary(error.range.end),
                "error range {:?} in {src:?}",
                error.range
            );
        }
        (tokens, markup)
    }

    fn lumen(src: &str) -> Vec<(TokenKind, &str)> {
        let (tokens, markup) = check_lumen(src);
        assert!(
            markup.errors.is_empty(),
            "unexpected markup errors in {src:?}: {:?}",
            markup.errors
        );
        tokens
            .into_iter()
            .filter(|token| !token.is_trivia() && token.kind != Eof)
            .map(|token| (token.kind, token.text(src)))
            .collect()
    }

    fn lumen_errors(src: &str) -> Vec<(std::string::String, &str)> {
        check_lumen(src)
            .1
            .errors
            .into_iter()
            .map(|error| (error.message, &src[error.range]))
            .collect()
    }

    #[test]
    fn lumen_comparisons_stay_lua() {
        assert_eq!(lumen("a < b"), vec![(Name, "a"), (Lt, "<"), (Name, "b")]);
        assert_eq!(lumen("x<y"), vec![(Name, "x"), (Lt, "<"), (Name, "y")]);
        assert_eq!(
            lumen("#list < 3"),
            vec![(Hash, "#"), (Name, "list"), (Lt, "<"), (Number, "3")]
        );
        assert_eq!(lumen("a <= b"), vec![(Name, "a"), (Le, "<="), (Name, "b")]);
        for src in [
            "f() <a", "t[1] <a", "{} <a", "1 <a", "'s' <a", "... <a", "nil <a", "true <a",
            "false <a", "end <a",
        ] {
            assert!(
                lumen(src).iter().all(|(kind, _)| *kind != Markup),
                "{src} has markup"
            );
            assert!(lumen(src).iter().any(|(kind, _)| *kind == Lt), "{src}");
        }
    }

    #[test]
    fn lumen_markup_in_expression_position() {
        assert_eq!(
            lumen("return <view/>"),
            vec![(Return, "return"), (Markup, "<view/>")]
        );
        assert_eq!(
            lumen("f(<a/>)"),
            vec![(Name, "f"), (LParen, "("), (Markup, "<a/>"), (RParen, ")")]
        );
        assert_eq!(
            lumen("{ <a/> }"),
            vec![(LBrace, "{"), (Markup, "<a/>"), (RBrace, "}")]
        );
        assert_eq!(
            lumen("x = cond and <a />"),
            vec![
                (Name, "x"),
                (Assign, "="),
                (Name, "cond"),
                (And, "and"),
                (Markup, "<a />")
            ]
        );
        assert_eq!(lumen("!<a/>"), vec![(Bang, "!"), (Markup, "<a/>")]);
        assert_eq!(lumen("<a/>"), vec![(Markup, "<a/>")]);
        assert_eq!(
            lumen("x = <a/> < 1"),
            vec![
                (Name, "x"),
                (Assign, "="),
                (Markup, "<a/>"),
                (Lt, "<"),
                (Number, "1")
            ]
        );
    }

    #[test]
    fn lumen_strings_and_comments_hide_markup() {
        assert_eq!(
            lumen("x = '<a/>' -- <b>"),
            vec![
                (Name, "x"),
                (Assign, "="),
                (String { long: false }, "'<a/>'"),
                (Comment { long: false }, "-- <b>")
            ]
        );
    }

    #[test]
    fn lumen_attribute_values() {
        assert_eq!(
            lumen(r#"return <a title="x > y" alt='it"s' b c="it's" d/>"#),
            vec![
                (Return, "return"),
                (Markup, r#"<a title="x > y" alt='it"s' b c="it's" d/>"#)
            ]
        );
        assert_eq!(
            lumen("return <a b={x} c={{ d = 1 }}/>"),
            vec![
                (Return, "return"),
                (Markup, "<a b={"),
                (Name, "x"),
                (Markup, "} c={"),
                (LBrace, "{"),
                (Name, "d"),
                (Assign, "="),
                (Number, "1"),
                (RBrace, "}"),
                (Markup, "}/>")
            ]
        );
        assert_eq!(
            lumen("return <a {...props} b/>"),
            vec![
                (Return, "return"),
                (Markup, "<a {..."),
                (Name, "props"),
                (Markup, "} b/>")
            ]
        );
    }

    #[test]
    fn lumen_text_is_markup() {
        assert_eq!(
            lumen("return <text>the end -- \"q\" 'q' } (x) then</text>"),
            vec![
                (Return, "return"),
                (Markup, "<text>the end -- \"q\" 'q' } (x) then</text>")
            ]
        );
        assert_eq!(
            lumen("return <text>Hi, {name}!</text>"),
            vec![
                (Return, "return"),
                (Markup, "<text>Hi, {"),
                (Name, "name"),
                (Markup, "}!</text>")
            ]
        );
    }

    #[test]
    fn lumen_fragments_comments_and_self_closing_tags() {
        assert_eq!(
            lumen("return <><a/><b /><!-- c { < --></>"),
            vec![(Return, "return"), (Markup, "<><a/><b /><!-- c { < --></>")]
        );
        assert_eq!(
            lumen("return <>\n  <!-- a\n  b -->\n</>"),
            vec![
                (Return, "return"),
                (Markup, "<>"),
                (Markup, "<!-- a\n  b -->"),
                (Markup, "</>")
            ]
        );
    }

    #[test]
    fn lumen_nested_markup_in_splices() {
        assert_eq!(
            lumen("return <view>{map(list, function(item) return <row key={item}/> end)}</view>"),
            vec![
                (Return, "return"),
                (Markup, "<view>{"),
                (Name, "map"),
                (LParen, "("),
                (Name, "list"),
                (Comma, ","),
                (Function, "function"),
                (LParen, "("),
                (Name, "item"),
                (RParen, ")"),
                (Return, "return"),
                (Markup, "<row key={"),
                (Name, "item"),
                (Markup, "}/>"),
                (End, "end"),
                (RParen, ")"),
                (Markup, "}</view>")
            ]
        );
        assert_eq!(
            lumen("return <a>{x and <b>{y < 2 and <c/>}</b>}</a>"),
            vec![
                (Return, "return"),
                (Markup, "<a>{"),
                (Name, "x"),
                (And, "and"),
                (Markup, "<b>{"),
                (Name, "y"),
                (Lt, "<"),
                (Number, "2"),
                (And, "and"),
                (Markup, "<c/>"),
                (Markup, "}</b>"),
                (Markup, "}</a>")
            ]
        );
    }

    #[test]
    fn lumen_multiline_markup_splits_at_line_breaks() {
        let src = "return <button key={id}\n  style={x} >\n  {label}  \n  Some text\n</button>\n";
        let (tokens, markup) = check_lumen(src);
        assert!(markup.errors.is_empty());
        let all: Vec<(TokenKind, &str)> = tokens
            .iter()
            .filter(|token| token.kind != Eof)
            .map(|token| (token.kind, token.text(src)))
            .collect();
        assert_eq!(
            all,
            vec![
                (Return, "return"),
                (Space, " "),
                (Markup, "<button key={"),
                (Name, "id"),
                (Markup, "}"),
                (Newline, "\n"),
                (Space, "  "),
                (Markup, "style={"),
                (Name, "x"),
                (Markup, "} >"),
                (Newline, "\n"),
                (Space, "  "),
                (Markup, "{"),
                (Name, "label"),
                (Markup, "}"),
                (Space, "  "),
                (Newline, "\n"),
                (Space, "  "),
                (Markup, "Some text"),
                (Newline, "\n"),
                (Markup, "</button>"),
                (Newline, "\n")
            ]
        );
        assert_eq!(markup.elements, vec![7..src.len() - 1]);
        let value = "return <a title=\"x\n  y\" />";
        let (tokens, _) = check_lumen(value);
        let markup_token = tokens
            .iter()
            .find(|token| token.kind == Markup)
            .expect("markup");
        assert_eq!(markup_token.text(value), "<a title=\"x\n  y\" />");
        assert_eq!((markup_token.line, markup_token.end_line), (1, 2));
    }

    #[test]
    fn lumen_records_tags_and_elements() {
        let src = "local a = <Card x={<b/>}>t</Card>\nreturn <Flux.Panels.Card/>";
        let (_, markup) = check_lumen(src);
        let tags: Vec<(&str, bool)> = markup
            .tags
            .iter()
            .map(|tag| (&src[tag.name.clone()], tag.closing))
            .collect();
        assert_eq!(
            tags,
            vec![
                ("Card", false),
                ("b", false),
                ("Card", true),
                ("Flux.Panels.Card", false)
            ]
        );
        let elements: Vec<&str> = markup
            .elements
            .iter()
            .map(|range| &src[range.clone()])
            .collect();
        assert_eq!(
            elements,
            vec!["<Card x={<b/>}>t</Card>", "<Flux.Panels.Card/>"]
        );
    }

    #[test]
    fn lumen_records_splice_ends() {
        let src = "return <a b={x}>{}\n  } text\n  {<c/>}</a>";
        let (_, markup) = check_lumen(src);
        let ends = [
            src.find("x}").expect("attribute") + 1,
            src.find("{}").expect("empty splice") + 1,
            src.find("/>}").expect("child") + 2,
        ];
        assert_eq!(markup.splice_ends, ends);
    }

    #[test]
    fn lumen_reports_malformed_markup_like_the_compiler() {
        assert_eq!(
            lumen_errors("return <view>\n  <text>x</text>\n"),
            vec![("'<view>' is never closed".to_owned(), "<view")]
        );
        assert_eq!(
            lumen_errors("return <view><b></text></view>")[0],
            ("'</b>' expected, got '</text'".to_owned(), "text")
        );
        assert_eq!(
            lumen_errors("return <>x"),
            vec![("the fragment is never closed".to_owned(), "<>")]
        );
        assert_eq!(
            lumen_errors("return <a b=c/>")[0],
            (
                "the value of the 'b' attribute must be quoted or in { }".to_owned(),
                "c"
            )
        );
        assert_eq!(
            lumen_errors("return <a b='c/>")[0],
            ("unfinished value of the 'b' attribute".to_owned(), "'")
        );
        assert_eq!(
            lumen_errors("return <a b={}/>"),
            vec![("the 'b' attribute has an empty expression".to_owned(), "{")]
        );
        assert_eq!(
            lumen_errors("return <a {...}/>"),
            vec![("spread expression expected after ...".to_owned(), "{...")]
        );
        assert_eq!(
            lumen_errors("return <a / >")[0],
            ("'>' expected after '/'".to_owned(), "/")
        );
        assert_eq!(
            lumen_errors("return <a 1/>")[0],
            ("attribute name expected in '<a'".to_owned(), "1")
        );
        assert_eq!(
            lumen_errors("return <a b={x")[0],
            ("unfinished expression: '}' expected".to_owned(), "{")
        );
        assert_eq!(
            lumen_errors("return <a><!-- x</a>")[0],
            ("unfinished comment".to_owned(), "<!--")
        );
        assert_eq!(
            lumen_errors("return <a>1 < 2</a>")[0],
            ("tag name expected".to_owned(), "<")
        );
        assert_eq!(
            lumen_errors("return <a></a x>")[0],
            ("'>' expected in the closing tag of 'a'".to_owned(), "x")
        );
        assert_eq!(
            lumen_errors("x = \u{e9} <a"),
            vec![("unfinished tag '<a'".to_owned(), "<a")]
        );
        assert!(lumen_errors("x = a\u{e9}b <a").is_empty());
        assert_eq!(
            lumen_errors("return <a\n"),
            vec![("unfinished tag '<a'".to_owned(), "<a")]
        );
        assert_eq!(
            lumen_errors("return <My-card/>"),
            vec![(
                "the name of the 'My-card' component must be a Lua expression and can not have \
                 '-' in it"
                    .to_owned(),
                "<My-card"
            )]
        );
        assert!(lumen_errors("return <my-card/>").is_empty());
        assert_eq!(
            lumen_errors("return <a é/>")[0],
            ("attribute name expected in '<a'".to_owned(), "é")
        );
        assert_eq!(
            lumen_errors("return <a></é>")[0],
            ("'</a>' expected, got '</é'".to_owned(), "é")
        );
    }

    #[test]
    fn lumen_lexing_never_fails() {
        let alphabet = [
            "", "<", ">", "/", "{", "}", "a", "A", "=", "'", "\"", "!", "-", "\n", " ", ".", "...",
            "<!--", "-->", "</", "\t", "é", "[[", "--",
        ];
        for a in alphabet {
            for b in alphabet {
                for c in alphabet {
                    for d in ["", "<", ">", "{", "}", "a", "\n", "'"] {
                        check_lumen(&format!("x = {a}{b}{c}{d}"));
                        check_lumen(&format!("{a}{b}{c}{d}"));
                    }
                }
            }
        }
    }

    #[test]
    fn lumen_deep_nesting_does_not_exhaust_the_stack() {
        let src = format!("return {}", "<a>{".repeat(5000));
        let (_, markup) = check_lumen(&src);
        assert_eq!(markup.errors[0].message, "markup nested too deeply");
        let src = format!("return {}", "<a>".repeat(5000));
        check_lumen(&src);
    }

    #[test]
    fn lua_files_never_produce_markup() {
        assert!(
            check_invariants("return <view/>")
                .iter()
                .all(|token| token.kind != Markup)
        );
    }

    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
    fn lexes_the_flux_corpus() {
        let root = std::path::Path::new("/home/luna/code/flux-ce");
        let mut count = 0;
        let mut lumen_files = 0;
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
            if path.extension().is_some_and(|ext| ext == "lumen") {
                let text = std::fs::read_to_string(path).expect("UTF-8 source");
                let (tokens, markup) = check_lumen(&text);
                assert!(
                    markup.errors.is_empty(),
                    "{}: {:?}",
                    path.display(),
                    markup.errors
                );
                assert!(tokens.iter().any(|token| token.kind == Markup));
                lumen_files += 1;
            }
        }
        assert!(count >= 356, "only {count} files found");
        assert!(lumen_files >= 5, "only {lumen_files} Lumen templates found");
    }
}
