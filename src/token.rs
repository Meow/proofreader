//! Tokens produced by the GLua [lexer](crate::lexer).

use std::ops::Range;

/// The kind of a lexed token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    /// An identifier; non-ASCII bytes are allowed, as in GMod.
    Name,
    /// A numeric literal such as `12`, `0xFF`, `.5` or `1.5e-3`.
    Number,
    /// A string literal; `long` for `[[...]]` / `[==[...]==]` strings.
    String {
        /// Whether this is a long bracket string.
        long: bool,
    },
    /// A comment (`--`, `//`, `--[[ ]]`, `/* */`); `long` for the block forms.
    Comment {
        /// Whether this is a block comment (`--[[ ]]` or `/* */`).
        long: bool,
    },
    /// `and`
    And,
    /// `break`
    Break,
    /// `continue` (GLua)
    Continue,
    /// `do`
    Do,
    /// `else`
    Else,
    /// `elseif`
    ElseIf,
    /// `end`
    End,
    /// `false`
    False,
    /// `for`
    For,
    /// `function`
    Function,
    /// `goto`
    Goto,
    /// `if`
    If,
    /// `in`
    In,
    /// `local`
    Local,
    /// `nil`
    Nil,
    /// `not`
    Not,
    /// `or`
    Or,
    /// `repeat`
    Repeat,
    /// `return`
    Return,
    /// `then`
    Then,
    /// `true`
    True,
    /// `until`
    Until,
    /// `while`
    While,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `^`
    Caret,
    /// `#`
    Hash,
    /// `==`
    Eq,
    /// `!=` or `~=`; check the token text to tell them apart.
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `=`
    Assign,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `;`
    Semicolon,
    /// `:`
    Colon,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `..`
    Concat,
    /// `...`
    Dots,
    /// `!` (GLua)
    Bang,
    /// A lone `~`.
    Tilde,
    /// `&&` (GLua)
    AndAnd,
    /// `||` (GLua)
    OrOr,
    /// A run of spaces (also form feeds, vertical tabs and lone carriage returns).
    Space,
    /// A run of tab characters.
    Tab,
    /// A single `\n`, including a directly preceding `\r`.
    Newline,
    /// Any byte sequence the lexer does not recognise, including a leading UTF-8 byte order mark.
    Unknown,
    /// The empty token at the end of the input.
    Eof,
}

impl TokenKind {
    /// Returns the keyword kind spelled by `word`, if it is a GLua keyword.
    pub fn keyword(word: &str) -> Option<TokenKind> {
        Some(match word {
            "and" => TokenKind::And,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "do" => TokenKind::Do,
            "else" => TokenKind::Else,
            "elseif" => TokenKind::ElseIf,
            "end" => TokenKind::End,
            "false" => TokenKind::False,
            "for" => TokenKind::For,
            "function" => TokenKind::Function,
            "goto" => TokenKind::Goto,
            "if" => TokenKind::If,
            "in" => TokenKind::In,
            "local" => TokenKind::Local,
            "nil" => TokenKind::Nil,
            "not" => TokenKind::Not,
            "or" => TokenKind::Or,
            "repeat" => TokenKind::Repeat,
            "return" => TokenKind::Return,
            "then" => TokenKind::Then,
            "true" => TokenKind::True,
            "until" => TokenKind::Until,
            "while" => TokenKind::While,
            _ => return None,
        })
    }

    /// Whether this kind is whitespace (`Space`, `Tab` or `Newline`).
    pub fn is_trivia(self) -> bool {
        matches!(self, TokenKind::Space | TokenKind::Tab | TokenKind::Newline)
    }

    /// Whether this kind is a reserved word.
    pub fn is_keyword(self) -> bool {
        matches!(
            self,
            TokenKind::And
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Do
                | TokenKind::Else
                | TokenKind::ElseIf
                | TokenKind::End
                | TokenKind::False
                | TokenKind::For
                | TokenKind::Function
                | TokenKind::Goto
                | TokenKind::If
                | TokenKind::In
                | TokenKind::Local
                | TokenKind::Nil
                | TokenKind::Not
                | TokenKind::Or
                | TokenKind::Repeat
                | TokenKind::Return
                | TokenKind::Then
                | TokenKind::True
                | TokenKind::Until
                | TokenKind::While
        )
    }

    /// Whether this kind is a comment of either form.
    pub fn is_comment(self) -> bool {
        matches!(self, TokenKind::Comment { .. })
    }

    /// Whether this kind is a string literal of either form.
    pub fn is_string(self) -> bool {
        matches!(self, TokenKind::String { .. })
    }
}

/// A lexed token: a kind plus its byte range and position in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset one past the last byte.
    pub end: usize,
    /// 1-based line of the first byte.
    pub line: u32,
    /// 1-based byte column of the first byte.
    pub col: u32,
    /// 1-based line of the last byte (equal to `line` for empty tokens).
    pub end_line: u32,
}

impl Token {
    /// Returns the source text covered by this token.
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src.get(self.start..self.end).unwrap_or("")
    }

    /// Returns the length of the token in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the token covers no bytes (only `Eof` does).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns the byte range of the token.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Whether the token is whitespace.
    pub fn is_trivia(&self) -> bool {
        self.kind.is_trivia()
    }

    /// Whether the token spans more than one line.
    pub fn is_multiline(&self) -> bool {
        self.end_line > self.line
    }
}
