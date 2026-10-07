//! A minimal tracker of brackets and block keywords, shared by the lint readers.
//!
//! It does not parse expressions; it only pairs `(`/`)`, `[`/`]`, `{`/`}`, block openers with
//! `end`, and `repeat` with `until`. `for`/`while` and their `do` share one `end`.

use crate::token::{Token, TokenKind};

/// A construct that waits for its closer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    /// `(`, closed by `)`.
    Paren,
    /// `[`, closed by `]`.
    Bracket,
    /// `{`, closed by `}`.
    Brace,
    /// `function`, closed by `end`.
    Function,
    /// `if`, closed by `end`.
    If,
    /// A standalone `do`, closed by `end`.
    Do,
    /// `for` or `while`; `awaiting_do` until its `do` is seen, then closed by `end`.
    Loop {
        /// Whether the loop header has not reached its `do` yet.
        awaiting_do: bool,
    },
    /// `repeat`, closed by `until`.
    Repeat,
}

impl Opener {
    /// The text of the token that closes this construct.
    pub fn closer(self) -> &'static str {
        match self {
            Opener::Paren => ")",
            Opener::Bracket => "]",
            Opener::Brace => "}",
            Opener::Loop { awaiting_do: true } => "do",
            Opener::Repeat => "until",
            Opener::Function | Opener::If | Opener::Do | Opener::Loop { .. } => "end",
        }
    }
}

/// An open construct and the token that opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// What was opened.
    pub opener: Opener,
    /// The opening token.
    pub token: Token,
}

/// What a token did to the nesting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The token neither opened nor closed anything.
    Plain,
    /// The token opened a construct, now on top of the stack.
    Opened,
    /// The token closed this construct, popped from the stack.
    Closed(Frame),
    /// The token is a closer (or `else`/`elseif`/`do`) that does not fit the innermost open
    /// construct, shown here (`None` when nothing is open). The stack is left unchanged.
    Unexpected(Option<Frame>),
}

/// The stack of open constructs.
#[derive(Debug, Default, Clone)]
pub struct Nesting {
    /// Open constructs, innermost last.
    pub frames: Vec<Frame>,
}

impl Nesting {
    /// The innermost open construct.
    pub fn top(&self) -> Option<&Frame> {
        self.frames.last()
    }

    /// Feeds the next code token (comments must be skipped by the caller).
    pub fn step(&mut self, token: &Token) -> Step {
        let opener = match token.kind {
            TokenKind::LParen => Opener::Paren,
            TokenKind::LBracket => Opener::Bracket,
            TokenKind::LBrace => Opener::Brace,
            TokenKind::Function => Opener::Function,
            TokenKind::If => Opener::If,
            TokenKind::For | TokenKind::While => Opener::Loop { awaiting_do: true },
            TokenKind::Repeat => Opener::Repeat,
            TokenKind::Do => return self.step_do(token),
            TokenKind::RParen => return self.close(|o| o == Opener::Paren),
            TokenKind::RBracket => return self.close(|o| o == Opener::Bracket),
            TokenKind::RBrace => return self.close(|o| o == Opener::Brace),
            TokenKind::Until => return self.close(|o| o == Opener::Repeat),
            TokenKind::End => {
                return self.close(|o| {
                    matches!(
                        o,
                        Opener::Function
                            | Opener::If
                            | Opener::Do
                            | Opener::Loop { awaiting_do: false }
                    )
                });
            }
            TokenKind::Else | TokenKind::ElseIf => {
                return match self.top() {
                    Some(frame) if frame.opener == Opener::If => Step::Plain,
                    other => Step::Unexpected(other.copied()),
                };
            }
            _ => return Step::Plain,
        };
        self.frames.push(Frame {
            opener,
            token: *token,
        });
        Step::Opened
    }

    /// Handles `do`: it completes a pending `for`/`while` header or opens a standalone block.
    fn step_do(&mut self, token: &Token) -> Step {
        if let Some(frame) = self.frames.last_mut()
            && frame.opener == (Opener::Loop { awaiting_do: true })
        {
            frame.opener = Opener::Loop { awaiting_do: false };
            return Step::Plain;
        }
        self.frames.push(Frame {
            opener: Opener::Do,
            token: *token,
        });
        Step::Opened
    }

    /// Pops the innermost construct when `fits` accepts it.
    fn close(&mut self, fits: impl Fn(Opener) -> bool) -> Step {
        match self.frames.last() {
            Some(frame) if fits(frame.opener) => {
                let frame = *frame;
                self.frames.pop();
                Step::Closed(frame)
            }
            other => Step::Unexpected(other.copied()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Source;

    fn final_depth(src: &str) -> (usize, usize) {
        let source = Source::new("t.lua", src);
        let mut nesting = Nesting::default();
        let mut unexpected = 0;
        for token in source.code_tokens().filter(|t| !t.kind.is_comment()) {
            if let Step::Unexpected(_) = nesting.step(token) {
                unexpected += 1;
            }
        }
        (nesting.frames.len(), unexpected)
    }

    #[test]
    fn balanced_code() {
        assert_eq!(
            final_depth(
                "for i = 1, 2 do while x do end end\nrepeat local t = { (1) } until t[1]\n\
                 do end\nif a then elseif b then else end\nlocal f = function() end\n"
            ),
            (0, 0)
        );
        assert_eq!(
            final_depth("while f(function() do end end) do end\n"),
            (0, 0)
        );
    }

    #[test]
    fn unbalanced_code() {
        assert_eq!(final_depth("function f()\n"), (1, 0));
        assert_eq!(final_depth("x = (1]\n"), (1, 1));
        assert_eq!(final_depth("end\n"), (0, 1));
        assert_eq!(final_depth("else\n"), (0, 1));
        assert_eq!(final_depth("for i = 1, 2 end\n"), (1, 1));
    }

    #[test]
    fn closers() {
        assert_eq!(Opener::Loop { awaiting_do: true }.closer(), "do");
        assert_eq!(Opener::Loop { awaiting_do: false }.closer(), "end");
        assert_eq!(Opener::Repeat.closer(), "until");
        assert_eq!(Opener::Brace.closer(), "}");
    }
}
