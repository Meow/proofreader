//! Block structure of a GLua source, shared by the indentation and blank-line readers.
//!
//! [`BlockStructure::new`] walks the code tokens of a [`Source`] once, keeping a stack of open
//! blocks (`function`, `if`/`elseif`/`else`, `for`, `while`, `repeat`, `do` and the bracket pairs)
//! and classifying every line as blank, comment-only, inside a multi-line string or comment, a
//! statement start, a continuation of the previous line, or a closer line.

use std::collections::HashSet;

use crate::offense::Edit;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// What construct a block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `function ... end`, named or anonymous.
    Function,
    /// The `if ... then` segment of an `if` statement.
    If,
    /// An `elseif ... then` segment.
    ElseIf,
    /// An `else` segment.
    Else,
    /// `for ... do ... end`.
    For,
    /// `while ... do ... end`.
    While,
    /// `repeat ... until`.
    Repeat,
    /// A standalone `do ... end`.
    Do,
    /// `( ... )`.
    Paren,
    /// `[ ... ]`.
    Bracket,
    /// `{ ... }`.
    Brace,
}

impl BlockKind {
    /// Whether this is a bracket pair rather than a keyword block.
    pub fn is_bracket(self) -> bool {
        matches!(
            self,
            BlockKind::Paren | BlockKind::Bracket | BlockKind::Brace
        )
    }

    /// Whether this is an `elseif` or `else` segment of an `if` statement.
    pub fn is_segment(self) -> bool {
        matches!(self, BlockKind::ElseIf | BlockKind::Else)
    }

    /// Whether `end` closes this kind of block.
    fn closed_by_end(self) -> bool {
        !self.is_bracket() && self != BlockKind::Repeat
    }
}

/// A keyword block or bracket pair.
#[derive(Debug, Clone)]
pub struct Block {
    /// What construct this is.
    pub kind: BlockKind,
    /// The opening token: the keyword, the bracket, or `elseif`/`else` for segments.
    pub opener: Token,
    /// The line whose indentation the body and the closer are measured against: the line of the
    /// opening keyword or bracket, and for `elseif`/`else` segments the line of the `if`.
    pub anchor_line: u32,
    /// The token after which the body begins (`then`, `do`, `else`, `repeat`, the `)` of the
    /// parameter list, or the bracket itself); `None` while the header is incomplete.
    pub body_start: Option<Token>,
    /// Whether `body_start` is the last code token on its line.
    pub opens_line: bool,
    /// The first line holding body code when the body starts on a new line.
    pub first_body_line: Option<u32>,
    /// The closing token (`end`, `until`, `elseif`, `else` or the closing bracket).
    pub closer: Option<Token>,
    /// The enclosing block.
    pub parent: Option<usize>,
    /// For the parameter list of a function, the index of that function's block.
    params_of: Option<usize>,
    /// Start offset of the first code token after `body_start`.
    body_next: Option<usize>,
}

impl Block {
    /// The line of the closing token.
    pub fn close_line(&self) -> Option<u32> {
        self.closer.map(|token| token.line)
    }

    /// Whether the block opens and closes on the same line.
    pub fn is_one_line(&self) -> bool {
        self.closer
            .is_some_and(|closer| closer.line == self.opener.line)
    }

    /// Whether the block closes on a later line than the one it opens on.
    pub fn is_multiline(&self) -> bool {
        self.closer
            .is_some_and(|closer| closer.line > self.opener.line)
    }
}

/// How a line relates to the statements around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineKind {
    /// Empty or whitespace only.
    #[default]
    Blank,
    /// No code, only comments (or unrecognised bytes).
    Comment,
    /// Starts inside a multi-line string or comment.
    Skipped,
    /// Starts a statement, or an item of a bracket whose opener ends its line.
    Statement,
    /// Continues the expression or header of an earlier line.
    Continuation,
    /// Starts with a closer: `end`, `until`, `else`, `elseif`, `)`, `]` or `}`.
    Closer,
}

/// What is known about one line.
#[derive(Debug, Clone, Default)]
pub struct LineInfo {
    /// The line's classification.
    pub kind: LineKind,
    /// Indexes of the blocks open at the start of the line, outermost first.
    pub stack: Vec<usize>,
    /// For closer lines, the block closed by the first token.
    pub closes: Option<usize>,
    /// The first code token (comments excluded) starting on the line.
    pub first: Option<Token>,
}

/// Per-line block structure of a source; see the module documentation.
#[derive(Debug, Clone)]
pub struct BlockStructure {
    blocks: Vec<Block>,
    lines: Vec<LineInfo>,
}

/// Whether `kind` starts a line as a closer.
pub fn is_closer(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::End
            | TokenKind::Until
            | TokenKind::Else
            | TokenKind::ElseIf
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
    )
}

/// Whether a statement can begin with a token of this kind.
fn can_start_statement(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Name
            | TokenKind::LParen
            | TokenKind::Local
            | TokenKind::Function
            | TokenKind::If
            | TokenKind::For
            | TokenKind::While
            | TokenKind::Repeat
            | TokenKind::Do
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
            | TokenKind::Goto
            | TokenKind::Semicolon
            | TokenKind::Colon
    )
}

/// Whether a line starting with `kind` continues the previous line's expression.
fn continues_expression(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Concat
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::Comma
            | TokenKind::Plus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Assign
            | TokenKind::Then
            | TokenKind::In
    )
}

/// Whether a line ending with `kind` leaves its expression or statement incomplete.
fn leaves_expression_open(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::And
            | TokenKind::Or
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Not
            | TokenKind::Bang
            | TokenKind::Hash
            | TokenKind::Tilde
            | TokenKind::Minus
            | TokenKind::Plus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::Assign
            | TokenKind::Concat
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::Local
            | TokenKind::Return
            | TokenKind::In
            | TokenKind::If
            | TokenKind::ElseIf
            | TokenKind::While
            | TokenKind::Until
            | TokenKind::For
            | TokenKind::Function
            | TokenKind::Goto
    )
}

/// The token walk that builds a [`BlockStructure`].
struct Builder<'a> {
    source: &'a Source,
    tokens: Vec<Token>,
    skipped: HashSet<u32>,
    blocks: Vec<Block>,
    stack: Vec<usize>,
    lines: Vec<LineInfo>,
    filled: u32,
}

impl Builder<'_> {
    /// Visits the code token at `index`, recording its line first when it starts one.
    fn visit(&mut self, index: usize) {
        let token = self.tokens[index];
        let first = index == 0 || self.tokens[index - 1].end_line < token.line;
        if first {
            self.fill_until(token.line);
            let kind = if self.skipped.contains(&token.line) {
                LineKind::Skipped
            } else {
                self.classify(index)
            };
            if let Some(info) = self.lines.get_mut(token.line as usize - 1) {
                *info = LineInfo {
                    kind,
                    stack: self.stack.clone(),
                    closes: None,
                    first: Some(token),
                };
            }
            self.filled = token.line;
        }
        let closed = self.process(index);
        if first
            && let Some(info) = self.lines.get_mut(token.line as usize - 1)
            && info.kind == LineKind::Closer
        {
            info.closes = closed;
        }
    }

    /// Records the lines after the last recorded one and before `line`, which hold no code start.
    fn fill_until(&mut self, line: u32) {
        for number in self.filled + 1..line {
            let kind = if self.skipped.contains(&number) {
                LineKind::Skipped
            } else if self.source.is_blank(number) {
                LineKind::Blank
            } else {
                LineKind::Comment
            };
            if let Some(info) = self.lines.get_mut(number as usize - 1) {
                *info = LineInfo {
                    kind,
                    stack: self.stack.clone(),
                    closes: None,
                    first: None,
                };
            }
        }
    }

    /// Classifies the line starting with the code token at `index`.
    fn classify(&self, index: usize) -> LineKind {
        let token = self.tokens[index];
        if is_closer(token.kind) {
            return LineKind::Closer;
        }
        let top = self.stack.last().map(|&block| &self.blocks[block]);
        if top.is_some_and(|block| {
            block.body_start.is_none() || (block.kind.is_bracket() && !block.opens_line)
        }) {
            return LineKind::Continuation;
        }
        let items = top.is_some_and(|block| block.kind.is_bracket());
        if let Some(prev) = index.checked_sub(1).map(|position| self.tokens[position]) {
            let label_end = prev.kind == TokenKind::Colon
                && index
                    .checked_sub(2)
                    .is_some_and(|position| self.tokens[position].kind == TokenKind::Colon);
            if (leaves_expression_open(prev.kind) && !label_end)
                || (prev.kind == TokenKind::Comma && !items)
            {
                return LineKind::Continuation;
            }
        }
        let label_start = token.kind == TokenKind::Colon
            && self
                .tokens
                .get(index + 1)
                .is_some_and(|next| next.kind == TokenKind::Colon);
        if (continues_expression(token.kind) && !label_start)
            || (!items && !can_start_statement(token.kind))
        {
            return LineKind::Continuation;
        }
        LineKind::Statement
    }

    /// Applies the code token at `index` to the block stack; returns the block it closed.
    fn process(&mut self, index: usize) -> Option<usize> {
        let token = self.tokens[index];
        match token.kind {
            TokenKind::Function => {
                self.push(BlockKind::Function, index, false);
            }
            TokenKind::If => {
                self.push(BlockKind::If, index, false);
            }
            TokenKind::For => {
                self.push(BlockKind::For, index, false);
            }
            TokenKind::While => {
                self.push(BlockKind::While, index, false);
            }
            TokenKind::Repeat => {
                self.push(BlockKind::Repeat, index, true);
            }
            TokenKind::ElseIf | TokenKind::Else => {
                let closed = self.close(index, |kind| {
                    matches!(kind, BlockKind::If | BlockKind::ElseIf)
                });
                let (kind, starts_body) = if token.kind == TokenKind::Else {
                    (BlockKind::Else, true)
                } else {
                    (BlockKind::ElseIf, false)
                };
                let pushed = self.push(kind, index, starts_body);
                if let Some(closed) = closed {
                    self.blocks[pushed].anchor_line = self.blocks[closed].anchor_line;
                }
                return closed;
            }
            TokenKind::Then => {
                if let Some(&top) = self.stack.last()
                    && matches!(self.blocks[top].kind, BlockKind::If | BlockKind::ElseIf)
                    && self.blocks[top].body_start.is_none()
                {
                    self.start_body(top, index);
                }
            }
            TokenKind::Do => {
                if let Some(&top) = self.stack.last()
                    && matches!(self.blocks[top].kind, BlockKind::For | BlockKind::While)
                    && self.blocks[top].body_start.is_none()
                {
                    self.start_body(top, index);
                } else {
                    self.push(BlockKind::Do, index, true);
                }
            }
            TokenKind::LParen => {
                let params_of = self.stack.last().copied().filter(|&top| {
                    let block = &self.blocks[top];
                    block.kind == BlockKind::Function
                        && block.body_start.is_none()
                        && !self
                            .stack
                            .iter()
                            .any(|&open| self.blocks[open].params_of == Some(top))
                });
                let pushed = self.push(BlockKind::Paren, index, true);
                self.blocks[pushed].params_of = params_of;
            }
            TokenKind::LBracket => {
                self.push(BlockKind::Bracket, index, true);
            }
            TokenKind::LBrace => {
                self.push(BlockKind::Brace, index, true);
            }
            TokenKind::End => return self.close(index, BlockKind::closed_by_end),
            TokenKind::Until => return self.close(index, |kind| kind == BlockKind::Repeat),
            TokenKind::RParen => {
                let closed = self.close(index, |kind| kind == BlockKind::Paren);
                if let Some(function) = closed.and_then(|paren| self.blocks[paren].params_of) {
                    self.start_body(function, index);
                }
                return closed;
            }
            TokenKind::RBracket => return self.close(index, |kind| kind == BlockKind::Bracket),
            TokenKind::RBrace => return self.close(index, |kind| kind == BlockKind::Brace),
            _ => {}
        }
        None
    }

    /// Opens a block of `kind` at the code token `index`, optionally starting its body there.
    fn push(&mut self, kind: BlockKind, index: usize, starts_body: bool) -> usize {
        let token = self.tokens[index];
        let block = self.blocks.len();
        self.blocks.push(Block {
            kind,
            opener: token,
            anchor_line: token.line,
            body_start: None,
            opens_line: false,
            first_body_line: None,
            closer: None,
            parent: self.stack.last().copied(),
            params_of: None,
            body_next: None,
        });
        self.stack.push(block);
        if starts_body {
            self.start_body(block, index);
        }
        block
    }

    /// Marks the code token `index` as the start of the body of `block`.
    fn start_body(&mut self, block: usize, index: usize) {
        let token = self.tokens[index];
        let next = self.tokens.get(index + 1);
        let opens_line = next.is_none_or(|next| next.line > token.end_line);
        let first_body_line = next
            .filter(|next| next.line > token.end_line && !self.skipped.contains(&next.line))
            .map(|next| next.line);
        let entry = &mut self.blocks[block];
        entry.body_start = Some(token);
        entry.opens_line = opens_line;
        entry.first_body_line = first_body_line;
        entry.body_next = next.map(|next| next.start);
    }

    /// Closes the innermost open block matching `matches` with the code token `index`, dropping
    /// any unclosed blocks above it; returns the closed block.
    fn close(&mut self, index: usize, matches: impl Fn(BlockKind) -> bool) -> Option<usize> {
        let position = self
            .stack
            .iter()
            .rposition(|&block| matches(self.blocks[block].kind))?;
        let block = self.stack[position];
        self.stack.truncate(position);
        let closer = self.tokens[index];
        let entry = &mut self.blocks[block];
        entry.closer = Some(closer);
        if entry.body_next == Some(closer.start) {
            entry.first_body_line = None;
        }
        Some(block)
    }
}

impl BlockStructure {
    /// Analyses `source`.
    pub fn new(source: &Source) -> Self {
        let tokens: Vec<Token> = source
            .code_tokens()
            .filter(|token| {
                !token.kind.is_comment()
                    && !matches!(token.kind, TokenKind::Eof | TokenKind::Unknown)
            })
            .copied()
            .collect();
        let count = source.line_count();
        let mut builder = Builder {
            source,
            tokens,
            skipped: source.lines_inside_multiline_tokens(),
            blocks: Vec::new(),
            stack: Vec::new(),
            lines: vec![LineInfo::default(); count as usize],
            filled: 0,
        };
        for index in 0..builder.tokens.len() {
            builder.visit(index);
        }
        builder.fill_until(count + 1);
        BlockStructure {
            blocks: builder.blocks,
            lines: builder.lines,
        }
    }

    /// Every block, in order of their opening tokens.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The block with index `index`.
    pub fn block(&self, index: usize) -> &Block {
        &self.blocks[index]
    }

    /// Information about line `n` (1-based); `None` when out of range.
    pub fn line(&self, n: u32) -> Option<&LineInfo> {
        n.checked_sub(1)
            .and_then(|index| self.lines.get(index as usize))
    }

    /// Number of lines.
    pub fn line_count(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }

    /// The classification of line `n`; `Blank` when out of range.
    pub fn kind(&self, n: u32) -> LineKind {
        self.line(n).map_or(LineKind::Blank, |info| info.kind)
    }

    /// Whether line `n` holds code (a statement, a continuation or a closer).
    pub fn has_code(&self, n: u32) -> bool {
        matches!(
            self.kind(n),
            LineKind::Statement | LineKind::Continuation | LineKind::Closer
        )
    }

    /// The first code token of line `n`.
    pub fn first_token(&self, n: u32) -> Option<Token> {
        self.line(n).and_then(|info| info.first)
    }

    /// The innermost block open at the start of line `n`; `None` at the top level.
    pub fn innermost(&self, n: u32) -> Option<usize> {
        self.line(n).and_then(|info| info.stack.last().copied())
    }

    /// Whether line `n` starts a statement inside a keyword block or at the top level, as opposed
    /// to an item inside a bracket.
    pub fn is_statement_start(&self, n: u32) -> bool {
        self.kind(n) == LineKind::Statement
            && self
                .innermost(n)
                .is_none_or(|block| !self.blocks[block].kind.is_bracket())
    }

    /// For a closer line, the block its first token closes.
    pub fn closed_block(&self, n: u32) -> Option<usize> {
        self.line(n).and_then(|info| info.closes)
    }

    /// For a closer line, the anchor line of the block its first token closes.
    pub fn opener_line_of_closer(&self, n: u32) -> Option<u32> {
        self.closed_block(n)
            .map(|block| self.blocks[block].anchor_line)
    }

    /// The line where the statement holding line `n` starts: `n` itself unless it is a
    /// continuation line, in which case the nearest earlier statement or closer line.
    pub fn statement_line(&self, n: u32) -> u32 {
        let mut line = n;
        while self.kind(line) != LineKind::Statement && self.kind(line) != LineKind::Closer {
            if line <= 1 {
                return n;
            }
            line -= 1;
        }
        line
    }

    /// Whether statement lines `upper` and `deeper` belong to the same block and `upper` is
    /// indented less than `deeper`: the edge of a visual group of statements indented one
    /// level deeper than their block (`net.Start(id)` / `  net.WriteString(s)` / `net.Send()`).
    pub fn is_group_edge(&self, source: &Source, upper: u32, deeper: u32) -> bool {
        self.kind(upper) == LineKind::Statement
            && self.kind(deeper) == LineKind::Statement
            && self.innermost(upper) == self.innermost(deeper)
            && matches!(
                (indent_width(source, upper), indent_width(source, deeper)),
                (Some(shallow), Some(deep)) if shallow < deep
            )
    }

    /// The continuation lines that follow line `n`, up to the next statement or closer line;
    /// blank, comment and skipped lines in between are passed over.
    pub fn continuation_lines(&self, n: u32) -> Vec<u32> {
        let mut lines = Vec::new();
        for line in n + 1..=self.line_count() {
            match self.kind(line) {
                LineKind::Continuation => lines.push(line),
                LineKind::Blank | LineKind::Comment | LineKind::Skipped => {}
                LineKind::Statement | LineKind::Closer => break,
            }
        }
        lines
    }

    /// The first keyword block (not a bracket, not an `elseif`/`else` segment) that opens on
    /// line `n` and closes on a later line.
    pub fn multiline_block_on_line(&self, n: u32) -> Option<usize> {
        self.multiline_blocks_on_line(n).next()
    }

    /// Every keyword block (not a bracket, not an `elseif`/`else` segment) that opens on line
    /// `n` and closes on a later line, in source order.
    pub fn multiline_blocks_on_line(&self, n: u32) -> impl Iterator<Item = usize> + '_ {
        let start = self.blocks.partition_point(|block| block.opener.line < n);
        self.blocks[start..]
            .iter()
            .take_while(move |block| block.opener.line == n)
            .enumerate()
            .filter(|(_, block)| {
                !block.kind.is_bracket() && !block.kind.is_segment() && block.is_multiline()
            })
            .map(move |(offset, _)| start + offset)
    }

    /// The `if` block of a one-line guard clause filling line `n`
    /// (`if ... then return|continue|break ... end`).
    pub fn guard_clause(&self, source: &Source, n: u32) -> Option<usize> {
        let first = self.first_token(n)?;
        if first.kind != TokenKind::If || self.kind(n) != LineKind::Statement {
            return None;
        }
        let index = self.block_opened_by(first)?;
        let block = &self.blocks[index];
        let closer = block.closer?;
        let body_start = block.body_start?;
        if closer.kind != TokenKind::End || closer.line != n {
            return None;
        }
        let after_then = next_code_token(source, body_start)?;
        let last = last_code_token_on_line(source, n)?;
        let exits = matches!(
            after_then.kind,
            TokenKind::Return | TokenKind::Continue | TokenKind::Break
        );
        (exits && last == closer).then_some(index)
    }

    /// The function block of a definition on line `n`: `function name(`, `function a.b:c(` or
    /// `local function name(` at the start of the line.
    pub fn definition(&self, source: &Source, n: u32) -> Option<usize> {
        let first = self.first_token(n)?;
        if self.kind(n) != LineKind::Statement {
            return None;
        }
        let keyword = match first.kind {
            TokenKind::Function => first,
            TokenKind::Local => {
                next_code_token(source, first).filter(|next| next.kind == TokenKind::Function)?
            }
            _ => return None,
        };
        let name = next_code_token(source, keyword)?;
        if name.kind != TokenKind::Name {
            return None;
        }
        self.block_opened_by(keyword)
    }

    /// The first line of the doc comment attached to the definition on line `n`: a run of
    /// comment-only lines directly above it whose first line starts with `---` and whose other
    /// lines start with `--`.
    pub fn doc_comment_start(&self, source: &Source, n: u32) -> Option<u32> {
        let mut line = n.checked_sub(1)?;
        while line >= 1 && self.kind(line) == LineKind::Comment {
            let text = source.line(line).trim_start();
            if text.starts_with("---") {
                return Some(line);
            }
            if !text.starts_with("--") {
                return None;
            }
            line -= 1;
        }
        None
    }

    /// The block opened by `token`.
    fn block_opened_by(&self, token: Token) -> Option<usize> {
        let index = self
            .blocks
            .partition_point(|block| block.opener.start < token.start);
        self.blocks
            .get(index)
            .filter(|block| block.opener.start == token.start)
            .map(|_| index)
    }

    /// The comment-only lines directly above line `n` (no blank line in between) that share its
    /// indentation, such as its doc comment, in top-down order.
    pub fn attached_comment_lines(&self, source: &Source, n: u32) -> Vec<u32> {
        let indentation = source.indentation(n);
        let mut lines: Vec<u32> = (1..n)
            .rev()
            .take_while(|&line| {
                self.kind(line) == LineKind::Comment && source.indentation(line) == indentation
            })
            .collect();
        lines.reverse();
        lines
    }

    /// Edits that re-indent line `n` to `width` spaces and shift its continuation lines and the
    /// comment lines attached above it by the same amount. Lines whose indentation holds tabs are
    /// left alone.
    ///
    /// Each edit replaces the indentation together with the line's first token, so that it
    /// conflicts with any other edit touching the start of that line.
    pub fn reindent(&self, source: &Source, n: u32, width: usize) -> Vec<Edit> {
        let current = source.indentation(n).len();
        let mut edits = Vec::new();
        let comments = self.attached_comment_lines(source, n);
        for line in comments
            .into_iter()
            .chain(std::iter::once(n))
            .chain(self.continuation_lines(n))
        {
            let indentation = source.indentation(line);
            if indentation.contains('\t') {
                continue;
            }
            let target = (indentation.len() + width).saturating_sub(current);
            if target == indentation.len() {
                continue;
            }
            if let Some(edit) = reindent_line(source, line, target) {
                edits.push(edit);
            }
        }
        edits
    }

    /// Edits that re-indent the first body line of `block` to `width` spaces and shift every
    /// other line of the body (comments, nested blocks and their closers included) by the same
    /// amount, so that the body keeps its shape and any column alignment inside it. Blank lines,
    /// lines inside multi-line tokens, lines whose indentation holds tabs and lines indented less
    /// than the first body line are left alone. Without a closer, only the first body line and
    /// its continuations move.
    pub fn reindent_body(&self, source: &Source, block: &Block, width: usize) -> Vec<Edit> {
        let (Some(first), Some(start), Some(end)) =
            (block.first_body_line, block.body_start, block.close_line())
        else {
            return block
                .first_body_line
                .map(|line| self.reindent(source, line, width))
                .unwrap_or_default();
        };
        let current = source.indentation(first).len();
        let mut edits = Vec::new();
        for line in start.line + 1..end {
            if matches!(self.kind(line), LineKind::Blank | LineKind::Skipped) {
                continue;
            }
            let indentation = source.indentation(line);
            if indentation.contains('\t') || indentation.len() < current {
                continue;
            }
            let target = indentation.len() + width - current;
            if target == indentation.len() {
                continue;
            }
            if let Some(edit) = reindent_line(source, line, target) {
                edits.push(edit);
            }
        }
        edits
    }
}

/// An edit that sets the indentation of line `n` to `width` spaces, covering its first token.
fn reindent_line(source: &Source, n: u32, width: usize) -> Option<Edit> {
    let start = source.line_range(n).start;
    let lead = source
        .tokens_on_line(n)
        .iter()
        .find(|token| !token.is_trivia() && token.kind != TokenKind::Eof)?;
    if lead.start != start + source.indentation(n).len() {
        return None;
    }
    Some(Edit::replace(
        start..lead.end,
        format!("{}{}", " ".repeat(width), source.text_of(lead)),
    ))
}

/// The first non-comment code token after `token`.
fn next_code_token(source: &Source, token: Token) -> Option<Token> {
    let index = source
        .code_token_indexes()
        .partition_point(|&position| source.tokens[position].start <= token.start);
    source.code_token_indexes()[index..]
        .iter()
        .map(|&position| source.tokens[position])
        .find(|next| !next.kind.is_comment())
        .filter(|next| next.kind != TokenKind::Eof)
}

/// The last non-comment code token ending on line `n`.
pub fn last_code_token_on_line(source: &Source, n: u32) -> Option<Token> {
    let start = source.tokens.partition_point(|token| token.end_line < n);
    let end = source.tokens.partition_point(|token| token.end_line <= n);
    source.tokens[start..end]
        .iter()
        .rev()
        .find(|token| {
            !token.is_trivia()
                && !token.kind.is_comment()
                && !matches!(token.kind, TokenKind::Eof | TokenKind::Unknown)
        })
        .copied()
}

/// An edit that adds `count` blank lines after line `n` by rewriting the line break ending it.
///
/// Readers that ask for a blank line at the same place produce the same edit, so the corrector
/// applies only one of them.
pub fn insert_blank_lines_after(source: &Source, n: u32, count: usize) -> Option<Edit> {
    let end = source.line_range(n).end;
    if source.text.as_bytes().get(end) != Some(&b'\n') {
        return None;
    }
    let newline = if source.line(n).ends_with('\r') {
        "\r\n"
    } else {
        "\n"
    };
    Some(Edit::replace(
        end..end + 1,
        format!("\n{}", newline.repeat(count)),
    ))
}

/// Width of the indentation of line `n`, or `None` when it contains tabs.
pub fn indent_width(source: &Source, n: u32) -> Option<usize> {
    let indentation = source.indentation(n);
    (!indentation.contains('\t')).then_some(indentation.len())
}

/// The byte range to report for an indentation problem on line `n`: the indentation, or the
/// first token when the line is not indented.
pub fn indentation_range(source: &Source, n: u32, first: Token) -> std::ops::Range<usize> {
    let start = source.line_range(n).start;
    let indentation = source.indentation(n).len();
    if indentation == 0 {
        first.range()
    } else {
        start..start + indentation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LineKind::{Blank, Closer, Comment, Continuation, Skipped, Statement};

    fn analyse(text: &str) -> (Source, BlockStructure) {
        let source = Source::new("test.lua", text);
        let structure = BlockStructure::new(&source);
        (source, structure)
    }

    fn kinds(text: &str) -> Vec<LineKind> {
        let (_, structure) = analyse(text);
        (1..=structure.line_count())
            .map(|line| structure.kind(line))
            .collect()
    }

    #[test]
    fn classifies_simple_statements_and_closers() {
        assert_eq!(
            kinds("local a = 1\n\nif a then\n  b()\nelse\n  c()\nend\n"),
            vec![
                Statement, Blank, Statement, Statement, Closer, Statement, Closer, Blank
            ]
        );
    }

    #[test]
    fn comments_and_multiline_tokens() {
        assert_eq!(
            kinds("-- note\nlocal s = [[\na\n  b]]\n--[[\nx\n]] y()\nz()"),
            vec![
                Comment, Statement, Skipped, Skipped, Comment, Skipped, Skipped, Statement
            ]
        );
    }

    #[test]
    fn trailing_operators_make_continuations() {
        assert_eq!(
            kinds("local a = b and\n  c\nlocal d =\n  1\nlocal s = 'a'..\n  'b'\nx()"),
            vec![
                Statement,
                Continuation,
                Statement,
                Continuation,
                Statement,
                Continuation,
                Statement
            ]
        );
        assert_eq!(
            kinds("local a,\n  b = 1, 2\nreturn\n  x"),
            vec![Statement, Continuation, Statement, Continuation]
        );
    }

    #[test]
    fn leading_operators_make_continuations() {
        assert_eq!(
            kinds(
                "local s = str\n  :gsub('a', 'b')\n  :lower()\nlocal x = a\n  and b\n  or c\ny = z\n  ..w"
            ),
            vec![
                Statement,
                Continuation,
                Continuation,
                Statement,
                Continuation,
                Continuation,
                Statement,
                Continuation
            ]
        );
        assert_eq!(
            kinds("return x\n  + 1\nreturn y\n  .field"),
            vec![Statement, Continuation, Statement, Continuation]
        );
    }

    #[test]
    fn values_cannot_start_statements() {
        assert_eq!(
            kinds("return\n  'x'\nf()\n  -1"),
            vec![Statement, Continuation, Statement, Continuation]
        );
    }

    #[test]
    fn open_brackets_that_do_not_end_their_line_hold_continuations() {
        assert_eq!(
            kinds("foo(a,\n    b,\n    c)\nbar()"),
            vec![Statement, Continuation, Continuation, Statement]
        );
    }

    #[test]
    fn line_ending_brackets_hold_items() {
        let (_, structure) = analyse("local t = {\n  'a',\n  'b'..\n    'c',\n  d = 1\n}\n");
        let found: Vec<LineKind> = (1..=6).map(|line| structure.kind(line)).collect();
        assert_eq!(
            found,
            vec![
                Statement,
                Statement,
                Statement,
                Continuation,
                Statement,
                Closer
            ]
        );
        assert!(!structure.is_statement_start(2));
        assert!(structure.is_statement_start(1));
        assert_eq!(structure.opener_line_of_closer(6), Some(1));
        let brace = structure.innermost(2).expect("inside the brace");
        assert_eq!(structure.block(brace).kind, BlockKind::Brace);
        assert!(structure.block(brace).opens_line);
        assert_eq!(structure.block(brace).first_body_line, Some(2));
    }

    #[test]
    fn multiline_headers_are_continuations() {
        let (_, structure) = analyse("if a and\n   b then\n  c()\nend\nwhile x\n  do\nend\n");
        let found: Vec<LineKind> = (1..=7).map(|line| structure.kind(line)).collect();
        assert_eq!(
            found,
            vec![
                Statement,
                Continuation,
                Statement,
                Closer,
                Statement,
                Continuation,
                Closer
            ]
        );
        let block = structure.innermost(3).expect("inside the if");
        assert_eq!(structure.block(block).kind, BlockKind::If);
        assert_eq!(structure.block(block).anchor_line, 1);
        assert_eq!(structure.block(block).first_body_line, Some(3));
        assert_eq!(structure.statement_line(2), 1);
    }

    #[test]
    fn function_parameters_and_bodies() {
        let (_, structure) = analyse("function foo(a,\n             b)\n  return a\nend\n");
        assert_eq!(structure.kind(2), Continuation);
        assert_eq!(structure.kind(3), Statement);
        let function = structure.innermost(3).expect("inside the function");
        let block = structure.block(function);
        assert_eq!(block.kind, BlockKind::Function);
        assert_eq!(block.body_start.map(|token| token.line), Some(2));
        assert!(block.opens_line);
        assert_eq!(block.first_body_line, Some(3));
        assert_eq!(block.close_line(), Some(4));
        assert_eq!(structure.opener_line_of_closer(4), Some(1));
    }

    #[test]
    fn nesting_stack() {
        let (_, structure) =
            analyse("hook.Add('X', 'y', function()\n  for i = 1, 2 do\n    x()\n  end\nend)\n");
        let stack = &structure.line(3).expect("line 3").stack;
        let found: Vec<BlockKind> = stack
            .iter()
            .map(|&block| structure.block(block).kind)
            .collect();
        assert_eq!(
            found,
            vec![BlockKind::Paren, BlockKind::Function, BlockKind::For]
        );
        assert_eq!(structure.kind(2), Statement);
        assert_eq!(structure.opener_line_of_closer(4), Some(2));
        assert_eq!(structure.opener_line_of_closer(5), Some(1));
        assert_eq!(structure.multiline_block_on_line(1), Some(1));
        assert!(structure.innermost(1).is_none());
    }

    #[test]
    fn else_chains_share_the_if_anchor() {
        let (_, structure) = analyse("if a then\n  b()\nelseif c then\n  d()\nelse\n  e()\nend\n");
        assert_eq!(structure.opener_line_of_closer(3), Some(1));
        assert_eq!(structure.opener_line_of_closer(5), Some(1));
        assert_eq!(structure.opener_line_of_closer(7), Some(1));
        let segment = structure.innermost(6).expect("inside else");
        assert_eq!(structure.block(segment).kind, BlockKind::Else);
        assert_eq!(structure.block(segment).first_body_line, Some(6));
        assert_eq!(structure.multiline_block_on_line(3), None);
        assert_eq!(structure.multiline_block_on_line(1), Some(0));
    }

    #[test]
    fn repeat_and_do_blocks() {
        let (_, structure) = analyse("repeat\n  x()\nuntil y\ndo\n  z()\nend\n");
        assert_eq!(structure.opener_line_of_closer(3), Some(1));
        assert_eq!(structure.opener_line_of_closer(6), Some(4));
        assert_eq!(structure.block(0).kind, BlockKind::Repeat);
        assert_eq!(structure.block(2).kind, BlockKind::Do);
        assert_eq!(structure.block(2).first_body_line, Some(5));
    }

    #[test]
    fn one_line_blocks() {
        let (source, structure) =
            analyse("if x then return end\nlocal f = function() return 1 end\n");
        assert!(structure.blocks().iter().all(Block::is_one_line));
        assert_eq!(structure.multiline_block_on_line(1), None);
        assert!(structure.guard_clause(&source, 1).is_some());
        assert!(structure.guard_clause(&source, 2).is_none());
        assert_eq!(structure.block(0).first_body_line, None);
    }

    #[test]
    fn guard_clauses() {
        let (source, structure) = analyse(
            "if !a then return end\nif b then continue end\nif c then break end\nif d then return false, 'x' end\nif e then f() end\nif g then return else h() end\nif i then return end x()\nif j then\n  return\nend\n",
        );
        let guards: Vec<bool> = (1..=10)
            .map(|line| structure.guard_clause(&source, line).is_some())
            .collect();
        assert_eq!(
            guards,
            vec![
                true, true, true, true, false, false, false, false, false, false
            ]
        );
    }

    #[test]
    fn definitions_and_doc_comments() {
        let (source, structure) = analyse(
            "--- Does a thing.\n-- @param x [Number]\nfunction a.b:c(x)\nend\n\nlocal function f()\nend\nlocal g = function()\nend\n-- plain\nfunction h() end\n",
        );
        assert!(structure.definition(&source, 3).is_some());
        assert!(structure.definition(&source, 6).is_some());
        assert!(structure.definition(&source, 8).is_none());
        assert!(structure.definition(&source, 11).is_some());
        assert_eq!(structure.doc_comment_start(&source, 3), Some(1));
        assert_eq!(structure.doc_comment_start(&source, 6), None);
        assert_eq!(structure.doc_comment_start(&source, 11), None);
    }

    #[test]
    fn labels_and_continue() {
        assert_eq!(
            kinds("for i = 1, 2 do\n  if i then continue end\n  ::skip::\n  x()\nend"),
            vec![Statement, Statement, Statement, Statement, Closer]
        );
    }

    #[test]
    fn empty_bodies_have_no_first_body_line() {
        let (_, structure) = analyse("function f()\nend\nlocal t = {\n}\n");
        assert_eq!(structure.block(0).first_body_line, None);
        assert_eq!(structure.blocks()[2].first_body_line, None);
    }

    #[test]
    fn tolerates_unbalanced_code() {
        let (_, structure) = analyse("end\n)\nif x then\n  (\nfunction\n");
        assert_eq!(structure.kind(1), Closer);
        assert!(structure.closed_block(1).is_none());
        assert_eq!(structure.line_count(), 6);
        let (_, structure) = analyse("");
        assert_eq!(structure.line_count(), 1);
        assert_eq!(structure.kind(1), Blank);
        assert_eq!(structure.kind(99), Blank);
    }

    #[test]
    fn statement_and_continuation_lines() {
        let (_, structure) = analyse("local x = foo(a,\n  b)\n  :c()\n\n-- note\ny()\n");
        assert_eq!(structure.continuation_lines(1), vec![2, 3]);
        assert_eq!(structure.statement_line(3), 1);
        assert_eq!(structure.statement_line(6), 6);
    }

    #[test]
    fn reindents_a_statement_with_its_continuations() {
        let (source, structure) = analyse("    local x = a and\n      b\nc()\n");
        let edits = structure.reindent(&source, 1, 2);
        assert_eq!(
            edits,
            vec![
                Edit::replace(0..9, "  local"),
                Edit::replace(20..27, "    b"),
            ]
        );
        let (source, structure) = analyse("x()\n  --- Doc.\n  -- More.\n  function f()\n  end\n");
        assert_eq!(structure.attached_comment_lines(&source, 4), vec![2, 3]);
        assert_eq!(
            structure.reindent(&source, 4, 0),
            vec![
                Edit::replace(4..14, "--- Doc."),
                Edit::replace(15..25, "-- More."),
                Edit::replace(26..36, "function"),
            ]
        );
        let (source, structure) = analyse("x()\n");
        assert_eq!(
            structure.reindent(&source, 1, 2),
            vec![Edit::replace(0..1, "  x")]
        );
    }

    #[test]
    fn reindents_a_whole_body() {
        let (source, structure) = analyse(
            "t = {\n    a   = 1,\n\n    -- c\n    bb  = {\n      d = 2\n    },\n  x = 3\n  }\n",
        );
        let block = &structure.blocks()[0];
        assert_eq!(
            structure.reindent_body(&source, block, 2),
            vec![
                Edit::replace(6..11, "  a"),
                Edit::replace(20..28, "  -- c"),
                Edit::replace(29..35, "  bb"),
                Edit::replace(41..48, "    d"),
                Edit::replace(53..58, "  }"),
            ]
        );
        let (source, structure) = analyse("if x then\n      y()\n    z()\nend\n");
        assert_eq!(
            structure.reindent_body(&source, &structure.blocks()[0], 2),
            vec![Edit::replace(10..17, "  y")]
        );
        let (source, structure) = analyse("if x then\n    y()\n");
        assert_eq!(
            structure.reindent_body(&source, &structure.blocks()[0], 2),
            vec![Edit::replace(10..15, "  y")]
        );
    }

    #[test]
    fn blank_line_edits() {
        let source = Source::new("t.lua", "a\nb\r\nc");
        assert_eq!(
            insert_blank_lines_after(&source, 1, 1),
            Some(Edit::replace(1..2, "\n\n"))
        );
        assert_eq!(
            insert_blank_lines_after(&source, 2, 2),
            Some(Edit::replace(4..5, "\n\r\n\r\n"))
        );
        assert_eq!(insert_blank_lines_after(&source, 3, 1), None);
    }

    #[test]
    fn indentation_helpers() {
        let source = Source::new("t.lua", "  x\n\ty\nz\n");
        assert_eq!(indent_width(&source, 1), Some(2));
        assert_eq!(indent_width(&source, 2), None);
        let first = source.first_code_token_on_line(3).copied().expect("z");
        assert_eq!(indentation_range(&source, 3, first), 7..8);
        let first = source.first_code_token_on_line(1).copied().expect("x");
        assert_eq!(indentation_range(&source, 1, first), 0..2);
    }

    #[test]
    fn last_code_token_skips_comments() {
        let source = Source::new("t.lua", "x = 1 -- c\n");
        let last = last_code_token_on_line(&source, 1).expect("token");
        assert_eq!(source.text_of(&last), "1");
    }

    /// Non-trivia tokens of `text` as `(kind, text)` pairs.
    fn code_sequence(text: &str) -> Vec<(TokenKind, String)> {
        let source = Source::new("t.lua", text);
        source
            .code_tokens()
            .map(|token| (token.kind, source.text_of(token).to_owned()))
            .collect()
    }

    #[test]
    #[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
    fn corrections_keep_the_code_of_the_flux_corpus() {
        use crate::config::Config;
        use crate::runner::{Options, ReaderFilter, inspect_source};
        let readers: Vec<String> = [
            "Layout/IndentationWidth",
            "Layout/IndentationConsistency",
            "Layout/EmptyLineBeforeBlock",
            "Layout/EmptyLineAfterBlock",
            "Layout/EmptyLineAfterGuardClause",
            "Layout/EmptyLineBetweenDefs",
        ]
        .map(String::from)
        .to_vec();
        let options = Options {
            fix: true,
            filter: ReaderFilter::new(&readers, &[]).expect("readers"),
            ..Options::default()
        };
        let config = Config::defaults(std::path::Path::new("."));
        let root = std::path::Path::new("/home/luna/code/flux-ce");
        let mut changed = 0;
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "lua")
                || path.starts_with(root.join(".git"))
            {
                continue;
            }
            let text = std::fs::read_to_string(path).expect("UTF-8 source");
            let (_, corrected) =
                inspect_source(&Source::new(path, text.as_str()), &config, &options);
            let Some(corrected) = corrected else {
                continue;
            };
            changed += 1;
            assert_eq!(
                code_sequence(&text),
                code_sequence(&corrected),
                "code changed in {}",
                path.display()
            );
            let (offenses, again) =
                inspect_source(&Source::new(path, corrected.as_str()), &config, &options);
            assert!(
                again.is_none(),
                "correction of {} is not idempotent",
                path.display()
            );
            assert!(offenses.is_empty(), "offenses left in {}", path.display());
        }
        assert!(changed > 0);
    }
}
