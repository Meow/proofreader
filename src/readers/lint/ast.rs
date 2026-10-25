//! A syntax tree of GLua, shared by the readers that need the structure of statements and
//! expressions rather than a token stream.
//!
//! [`parse`] builds the tree with a recursive descent parser over the code tokens (comments
//! dropped). It knows the GLua extensions (`!`, `!=`, `&&`, `||`, `continue`, `goto` and labels)
//! and, in Lumen templates, treats a markup element as an operand whose splices hold Lua
//! expressions. Tokens the grammar has no place for that are not keywords, operators or
//! punctuation (unrecognised bytes) are opaque values, as they are reported by `Lint/Syntax`
//! already.
//!
//! A file that does not parse yields a [`ParseError`] at the first token the grammar rejects.
//! The parser follows Lua's rules where they matter for reporting: `return` must be the last
//! statement of its block, a call or an assignment is the only expression that can stand as a
//! statement, and only names, fields and indexes can be assigned to.

use std::ops::Range;

use crate::lumen::opens_splice;
use crate::readers::naming::declarations::significant_tokens;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// The UTF-8 byte order mark, which the lexer keeps as an `Unknown` token.
const BOM: &str = "\u{feff}";

/// Deepest nesting of blocks and expressions the parser follows before giving up on a file.
const MAX_DEPTH: usize = 100;

/// A sequence of statements: a whole file, or the body of a function, loop or branch.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The statements in source order.
    pub statements: Vec<Statement>,
    /// From the first token of the first statement to the end of the last one; an empty range
    /// where the block would start when it has no statements.
    pub range: Range<usize>,
}

/// A statement and the bytes it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// What the statement is.
    pub kind: StatementKind,
    /// From its first token to the end of its last token (a separating `;` is its own
    /// [`StatementKind::Empty`] statement).
    pub range: Range<usize>,
}

/// The kinds of statements.
#[derive(Debug, Clone, PartialEq)]
pub enum StatementKind {
    /// A lone `;`.
    Empty,
    /// `local a, b = x, y`; `values` is empty without `=`.
    Local {
        /// The `local` keyword.
        local: Token,
        /// The declared names.
        names: Vec<Token>,
        /// The values after `=`.
        values: Vec<Expression>,
    },
    /// `local function f() ... end`.
    LocalFunction {
        /// The `local` keyword.
        local: Token,
        /// The declared name.
        name: Token,
        /// The function.
        function: Function,
    },
    /// `function a.b:c() ... end`.
    Function {
        /// The name the function is assigned to.
        name: FunctionName,
        /// The function.
        function: Function,
    },
    /// `a, b.c, d[1] = x, y, z`.
    Assignment {
        /// The assigned names, fields and indexes.
        targets: Vec<Expression>,
        /// The `=`.
        assign: Token,
        /// The assigned values.
        values: Vec<Expression>,
    },
    /// A function or method call standing as a statement; always a call expression.
    Call(Expression),
    /// `do ... end`.
    Do {
        /// The body.
        body: Block,
    },
    /// `while condition do ... end`.
    While {
        /// The loop condition.
        condition: Expression,
        /// The body.
        body: Block,
    },
    /// `repeat ... until condition`; the condition sees the locals of the body.
    Repeat {
        /// The body.
        body: Block,
        /// The `until` condition.
        condition: Expression,
    },
    /// `if ... then ... elseif ... then ... else ... end`.
    If {
        /// The `if` clause followed by every `elseif` clause.
        clauses: Vec<Clause>,
        /// The `else` keyword and the body of the `else` branch.
        otherwise: Option<(Token, Block)>,
        /// The closing `end`.
        end: Token,
    },
    /// `for i = start, limit, step do ... end`.
    NumericFor {
        /// The loop variable.
        variable: Token,
        /// The start value.
        start: Expression,
        /// The limit.
        limit: Expression,
        /// The step, if given.
        step: Option<Expression>,
        /// The body.
        body: Block,
    },
    /// `for k, v in values do ... end`.
    GenericFor {
        /// The loop variables.
        variables: Vec<Token>,
        /// The expressions after `in`.
        values: Vec<Expression>,
        /// The body.
        body: Block,
    },
    /// `return a, b`.
    Return {
        /// The `return` keyword.
        keyword: Token,
        /// The returned values.
        values: Vec<Expression>,
    },
    /// `break`.
    Break,
    /// `continue` (GLua).
    Continue,
    /// `goto label`.
    Goto {
        /// The label name.
        label: Token,
    },
    /// `::label::`.
    Label {
        /// The label name.
        name: Token,
    },
}

/// One `if` or `elseif` clause.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    /// The `if` or `elseif` keyword.
    pub keyword: Token,
    /// The condition.
    pub condition: Expression,
    /// The body.
    pub body: Block,
}

/// The name of a `function a.b:c()` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionName {
    /// The dotted names (`a`, `b`).
    pub path: Vec<Token>,
    /// The method name after `:` (`c`).
    pub method: Option<Token>,
}

/// A function: the parameter list and body shared by every function form.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The `function` keyword.
    pub keyword: Token,
    /// The named parameters, without the implicit `self` of a method.
    pub parameters: Vec<Token>,
    /// The `...` ending the parameter list, if any.
    pub vararg: Option<Token>,
    /// The `(` of the parameter list.
    pub open: Token,
    /// The `)` of the parameter list.
    pub close: Token,
    /// The body.
    pub body: Block,
    /// The closing `end`.
    pub end: Token,
}

/// An expression and the bytes it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    /// What the expression is.
    pub kind: ExpressionKind,
    /// From its first token to the end of its last token.
    pub range: Range<usize>,
}

/// The kinds of expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionKind {
    /// `nil`.
    Nil,
    /// `true`.
    True,
    /// `false`.
    False,
    /// A numeric literal.
    Number,
    /// A string literal.
    String(Token),
    /// `...`.
    Vararg,
    /// An anonymous function.
    Function(Box<Function>),
    /// A table constructor.
    Table(Table),
    /// A Lumen markup element with the expressions spliced into it.
    Markup(Vec<Expression>),
    /// A token the grammar does not know, such as an unrecognised byte.
    Opaque,
    /// A variable name.
    Name(Token),
    /// `(expression)`.
    Parenthesized(Box<Expression>),
    /// `object.name`.
    Field {
        /// The indexed expression.
        object: Box<Expression>,
        /// The field name.
        name: Token,
    },
    /// `object[key]`.
    Index {
        /// The indexed expression.
        object: Box<Expression>,
        /// The key.
        key: Box<Expression>,
    },
    /// `callee(arguments)`, `callee 'string'` or `callee { table }`.
    Call {
        /// The called expression.
        callee: Box<Expression>,
        /// The arguments.
        arguments: Arguments,
    },
    /// `object:name(arguments)`.
    MethodCall {
        /// The receiver.
        object: Box<Expression>,
        /// The method name.
        name: Token,
        /// The arguments.
        arguments: Arguments,
    },
    /// A unary operation (`-`, `#`, `not`, `!`, `~`).
    Unary {
        /// The operator.
        operator: Token,
        /// The operand.
        operand: Box<Expression>,
    },
    /// A binary operation.
    Binary {
        /// The operator.
        operator: Token,
        /// The left operand.
        left: Box<Expression>,
        /// The right operand.
        right: Box<Expression>,
    },
}

/// The arguments of a call.
#[derive(Debug, Clone, PartialEq)]
pub struct Arguments {
    /// How the arguments are written.
    pub style: ArgumentStyle,
    /// The argument values; a single table or string for those styles.
    pub values: Vec<Expression>,
    /// From the `(` (or the table or string) to the `)` (or the end of the table or string).
    pub range: Range<usize>,
}

/// How the arguments of a call are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentStyle {
    /// `f(a, b)`.
    Parenthesized,
    /// `f { ... }`.
    Table,
    /// `f 'text'`.
    String,
}

/// A table constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The `{`.
    pub open: Token,
    /// The fields in source order.
    pub fields: Vec<TableField>,
    /// The `}`.
    pub close: Token,
}

/// One field of a table constructor.
#[derive(Debug, Clone, PartialEq)]
pub struct TableField {
    /// How the field is keyed.
    pub key: FieldKey,
    /// The value.
    pub value: Expression,
    /// The `,` or `;` after the field, if any.
    pub separator: Option<Token>,
}

/// The key of a table field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKey {
    /// A list item: `{ value }`.
    Positional,
    /// `{ name = value }`.
    Named(Token),
    /// `{ [key] = value }`.
    Bracketed(Expression),
}

/// Why a file does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// The rejected token (an empty range at the end of the text for the end of the file).
    pub range: Range<usize>,
    /// What went wrong, as a sentence.
    pub message: String,
    /// Whether the parser gave up because the code nests deeper than it follows, rather than
    /// because the code is malformed.
    pub too_deep: bool,
}

/// Parses `source` into the block of its statements.
pub fn parse(source: &Source) -> Result<Block, ParseError> {
    let tokens: Vec<Token> = significant_tokens(source)
        .into_iter()
        .filter(|token| !(token.kind == TokenKind::Unknown && source.text_of(token) == BOM))
        .collect();
    let mut parser = Parser {
        source,
        tokens,
        pos: 0,
        depth: 0,
    };
    let block = parser.block()?;
    match parser.tokens.get(parser.pos) {
        None => Ok(block),
        Some(token) => Err(parser.unexpected(*token, "the end of the file")),
    }
}

impl Block {
    /// Whether the block has no statements other than `;`.
    pub fn is_empty(&self) -> bool {
        self.statements
            .iter()
            .all(|statement| statement.kind == StatementKind::Empty)
    }
}

impl Expression {
    /// The expression inside any number of parentheses.
    pub fn unparenthesized(&self) -> &Expression {
        match &self.kind {
            ExpressionKind::Parenthesized(inner) => inner.unparenthesized(),
            _ => self,
        }
    }

    /// Whether the expression is a call, which can produce any number of values.
    pub fn is_call(&self) -> bool {
        matches!(
            self.kind,
            ExpressionKind::Call { .. } | ExpressionKind::MethodCall { .. }
        )
    }

    /// Whether the expression can produce a number of values other than one: a call or `...`.
    pub fn is_multi_valued(&self) -> bool {
        self.is_call() || self.kind == ExpressionKind::Vararg
    }

    /// The dotted name of a name or field chain (`util.AddNetworkString`), if it is one.
    pub fn dotted_name(&self, source: &Source) -> Option<String> {
        match &self.kind {
            ExpressionKind::Name(token) => Some(source.text_of(token).to_owned()),
            ExpressionKind::Field { object, name } => {
                let mut path = object.dotted_name(source)?;
                path.push('.');
                path.push_str(source.text_of(name));
                Some(path)
            }
            _ => None,
        }
    }

    /// For a call of a plain function (`f(...)`, `a.b(...)`), its dotted name and arguments.
    pub fn as_call(&self, source: &Source) -> Option<(String, &Arguments)> {
        match &self.kind {
            ExpressionKind::Call { callee, arguments } => {
                Some((callee.dotted_name(source)?, arguments))
            }
            _ => None,
        }
    }

    /// The contents of a string literal without escapes, for comparing literal names: the text
    /// between the quotes (escapes kept as written) or between the long brackets.
    pub fn string_contents<'a>(&self, source: &'a Source) -> Option<&'a str> {
        let ExpressionKind::String(token) = &self.kind else {
            return None;
        };
        let text = source.text_of(token);
        if let Some(quote) = text.chars().next().filter(|c| *c == '\'' || *c == '"') {
            return text.strip_prefix(quote)?.strip_suffix(quote);
        }
        let level = text.strip_prefix('[')?;
        let equals = level.len() - level.trim_start_matches('=').len();
        let open = equals + 2;
        let close = open;
        let inner = text.get(open..text.len().checked_sub(close)?)?;
        Some(inner.strip_prefix('\n').unwrap_or(inner))
    }
}

/// Callbacks for [`walk`]; every method does nothing by default.
pub trait Visitor<'t> {
    /// Called before the statements of a block (a file, a function body or a loop or branch
    /// body) are walked.
    fn enter_block(&mut self, _block: &'t Block) {}

    /// Called after the statements of a block have been walked.
    fn leave_block(&mut self, _block: &'t Block) {}

    /// Called before the parameters and body of a function are walked.
    fn enter_function(&mut self, _function: &'t Function) {}

    /// Called after the body of a function has been walked.
    fn leave_function(&mut self, _function: &'t Function) {}

    /// Called for every statement before its parts are walked.
    fn statement(&mut self, _statement: &'t Statement) {}

    /// Called for every expression before its parts are walked.
    fn expression(&mut self, _expression: &'t Expression) {}
}

/// Walks `block` and everything in it in source order, calling `visitor` on the way. The
/// condition of a `repeat` loop is walked after its body has been left.
pub fn walk<'t>(block: &'t Block, visitor: &mut impl Visitor<'t>) {
    visitor.enter_block(block);
    for statement in &block.statements {
        walk_statement(statement, visitor);
    }
    visitor.leave_block(block);
}

/// Walks one statement.
fn walk_statement<'t>(statement: &'t Statement, visitor: &mut impl Visitor<'t>) {
    visitor.statement(statement);
    match &statement.kind {
        StatementKind::Empty
        | StatementKind::Break
        | StatementKind::Continue
        | StatementKind::Goto { .. }
        | StatementKind::Label { .. } => {}
        StatementKind::Local { values, .. } | StatementKind::Return { values, .. } => {
            walk_expressions(values, visitor);
        }
        StatementKind::LocalFunction { function, .. }
        | StatementKind::Function { function, .. } => {
            walk_function(function, visitor);
        }
        StatementKind::Assignment {
            targets, values, ..
        } => {
            walk_expressions(targets, visitor);
            walk_expressions(values, visitor);
        }
        StatementKind::Call(call) => walk_expression(call, visitor),
        StatementKind::Do { body } => walk(body, visitor),
        StatementKind::While { condition, body } => {
            walk_expression(condition, visitor);
            walk(body, visitor);
        }
        StatementKind::Repeat { body, condition } => {
            walk(body, visitor);
            walk_expression(condition, visitor);
        }
        StatementKind::If {
            clauses, otherwise, ..
        } => {
            for clause in clauses {
                walk_expression(&clause.condition, visitor);
                walk(&clause.body, visitor);
            }
            if let Some((_, body)) = otherwise {
                walk(body, visitor);
            }
        }
        StatementKind::NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            walk_expression(start, visitor);
            walk_expression(limit, visitor);
            if let Some(step) = step {
                walk_expression(step, visitor);
            }
            walk(body, visitor);
        }
        StatementKind::GenericFor { values, body, .. } => {
            walk_expressions(values, visitor);
            walk(body, visitor);
        }
    }
}

/// Walks a function: its body between [`Visitor::enter_function`] and
/// [`Visitor::leave_function`].
fn walk_function<'t>(function: &'t Function, visitor: &mut impl Visitor<'t>) {
    visitor.enter_function(function);
    walk(&function.body, visitor);
    visitor.leave_function(function);
}

/// Walks a list of expressions.
fn walk_expressions<'t>(expressions: &'t [Expression], visitor: &mut impl Visitor<'t>) {
    for expression in expressions {
        walk_expression(expression, visitor);
    }
}

/// Walks one expression.
fn walk_expression<'t>(expression: &'t Expression, visitor: &mut impl Visitor<'t>) {
    visitor.expression(expression);
    match &expression.kind {
        ExpressionKind::Nil
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::Number
        | ExpressionKind::String(_)
        | ExpressionKind::Vararg
        | ExpressionKind::Opaque
        | ExpressionKind::Name(_) => {}
        ExpressionKind::Function(function) => walk_function(function, visitor),
        ExpressionKind::Table(table) => {
            for field in &table.fields {
                if let FieldKey::Bracketed(key) = &field.key {
                    walk_expression(key, visitor);
                }
                walk_expression(&field.value, visitor);
            }
        }
        ExpressionKind::Markup(splices) => walk_expressions(splices, visitor),
        ExpressionKind::Parenthesized(inner) => walk_expression(inner, visitor),
        ExpressionKind::Field { object, .. } => walk_expression(object, visitor),
        ExpressionKind::Index { object, key } => {
            walk_expression(object, visitor);
            walk_expression(key, visitor);
        }
        ExpressionKind::Call { callee, arguments } => {
            walk_expression(callee, visitor);
            walk_expressions(&arguments.values, visitor);
        }
        ExpressionKind::MethodCall {
            object, arguments, ..
        } => {
            walk_expression(object, visitor);
            walk_expressions(&arguments.values, visitor);
        }
        ExpressionKind::Unary { operand, .. } => walk_expression(operand, visitor),
        ExpressionKind::Binary { left, right, .. } => {
            walk_expression(left, visitor);
            walk_expression(right, visitor);
        }
    }
}

/// The result of a parsing step.
type Parse<T> = Result<T, ParseError>;

/// Parser state: the tokens and the cursor.
struct Parser<'a> {
    source: &'a Source,
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

/// Whether `kind` has a fixed role in the grammar: keywords, operators, punctuation, whitespace,
/// comments and the end of the input. Every other kind (names, literals, markup and unrecognised
/// bytes) can stand for a value.
fn is_structural(kind: TokenKind) -> bool {
    kind.is_keyword()
        || binary_precedence(kind).is_some()
        || matches!(
            kind,
            TokenKind::Hash
                | TokenKind::Assign
                | TokenKind::LParen
                | TokenKind::RParen
                | TokenKind::LBrace
                | TokenKind::RBrace
                | TokenKind::LBracket
                | TokenKind::RBracket
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::Comma
                | TokenKind::Dot
                | TokenKind::Dots
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::Space
                | TokenKind::Tab
                | TokenKind::Newline
                | TokenKind::Comment { .. }
                | TokenKind::Eof
        )
}

/// Whether a token of `kind` can start a suffixed expression: a name, `(` or an opaque value.
fn starts_suffixed(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Name | TokenKind::LParen)
        || (!is_structural(kind) && !kind.is_string() && kind != TokenKind::Number)
}

/// The left and right binding power of a binary operator, as in Lua's precedence table: `or`,
/// `and`, comparisons, `..` (right associative), `+ -`, `* / %`, then `^` (right associative)
/// above the unary operators.
fn binary_precedence(kind: TokenKind) -> Option<(u8, u8)> {
    Some(match kind {
        TokenKind::Or | TokenKind::OrOr => (1, 1),
        TokenKind::And | TokenKind::AndAnd => (2, 2),
        TokenKind::Eq
        | TokenKind::Ne
        | TokenKind::Lt
        | TokenKind::Le
        | TokenKind::Gt
        | TokenKind::Ge => (3, 3),
        TokenKind::Concat => (5, 4),
        TokenKind::Plus | TokenKind::Minus => (6, 6),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (7, 7),
        TokenKind::Caret => (10, 9),
        _ => return None,
    })
}

/// Binding power of the operand of a unary operator.
const UNARY_PRECEDENCE: u8 = 8;

/// Whether `kind` is a unary operator.
fn is_unary_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Not | TokenKind::Minus | TokenKind::Hash | TokenKind::Bang | TokenKind::Tilde
    )
}

/// Whether `kind` ends a block.
fn ends_block(kind: Option<TokenKind>) -> bool {
    matches!(
        kind,
        None | Some(TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until)
    )
}

impl Parser<'_> {
    /// The kind of the token `ahead` positions after the cursor.
    fn peek_at(&self, ahead: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + ahead).map(|token| token.kind)
    }

    /// The kind of the token at the cursor.
    fn peek(&self) -> Option<TokenKind> {
        self.peek_at(0)
    }

    /// Whether the token at the cursor is of `kind`.
    fn check(&self, kind: TokenKind) -> bool {
        self.peek() == Some(kind)
    }

    /// The token at the cursor.
    fn current(&self) -> Option<Token> {
        self.tokens.get(self.pos).copied()
    }

    /// The end of the token before the cursor (the start of the text before the first token).
    fn last_end(&self) -> usize {
        self.pos
            .checked_sub(1)
            .and_then(|index| self.tokens.get(index))
            .map_or(0, |token| token.end)
    }

    /// Byte offset of the token at the cursor, or the end of the text.
    fn here(&self) -> usize {
        self.current()
            .map_or(self.source.text.len(), |token| token.start)
    }

    /// The error for `token` where `expected` should be.
    fn unexpected(&self, token: Token, expected: &str) -> ParseError {
        ParseError {
            range: token.range(),
            message: format!(
                "Unexpected `{}`; expected {expected}.",
                self.source.text_of(&token)
            ),
            too_deep: false,
        }
    }

    /// The error at the cursor where `expected` should be.
    fn error(&self, expected: &str) -> ParseError {
        match self.current() {
            Some(token) => self.unexpected(token, expected),
            None => {
                let end = self.source.text.len();
                ParseError {
                    range: end..end,
                    message: format!("Unexpected end of file; expected {expected}."),
                    too_deep: false,
                }
            }
        }
    }

    /// Consumes the token at the cursor.
    fn bump(&mut self) -> Parse<Token> {
        let token = self.current().ok_or_else(|| self.error("more code"))?;
        self.pos += 1;
        Ok(token)
    }

    /// Consumes the token at the cursor when it is of `kind`.
    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.check(kind) {
            self.pos += 1;
            return self.tokens.get(self.pos - 1).copied();
        }
        None
    }

    /// Consumes a token of `kind`, described as `what` in the error when the cursor is on
    /// anything else.
    fn expect(&mut self, kind: TokenKind, what: &str) -> Parse<Token> {
        self.eat(kind).ok_or_else(|| self.error(what))
    }

    /// Consumes the `end` closing the construct opened by `opener`.
    fn expect_end(&mut self, opener: &Token) -> Parse<Token> {
        let what = format!(
            "`end` to close `{}` on line {}",
            self.source.text_of(opener),
            opener.line
        );
        self.expect(TokenKind::End, &what)
    }

    /// Enters one more level of nesting, failing past [`MAX_DEPTH`].
    fn descend(&mut self) -> Parse<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            let start = self.here();
            return Err(ParseError {
                range: start..start,
                message: "The code nests too deeply to be inspected.".to_owned(),
                too_deep: true,
            });
        }
        Ok(())
    }

    /// Leaves a level of nesting entered with [`Parser::descend`].
    fn ascend(&mut self) {
        self.depth -= 1;
    }

    /// Parses statements up to a block terminator (`end`, `else`, `elseif`, `until` or the end
    /// of the file), which is left for the caller.
    fn block(&mut self) -> Parse<Block> {
        self.descend()?;
        let start = self.here();
        let mut statements = Vec::new();
        while !ends_block(self.peek()) {
            let statement = self.statement()?;
            let returns = matches!(statement.kind, StatementKind::Return { .. });
            statements.push(statement);
            if returns {
                if let Some(semicolon) = self.eat(TokenKind::Semicolon) {
                    statements.push(Statement {
                        kind: StatementKind::Empty,
                        range: semicolon.range(),
                    });
                }
                if !ends_block(self.peek()) {
                    return Err(self.error("the end of the block after `return`"));
                }
            }
        }
        self.ascend();
        let end = statements
            .last()
            .map_or(start, |statement: &Statement| statement.range.end);
        Ok(Block {
            statements,
            range: start..end.max(start),
        })
    }

    /// Wraps `kind` into a statement starting at `start` and ending with the previous token.
    fn finish(&self, kind: StatementKind, start: usize) -> Statement {
        Statement {
            kind,
            range: start..self.last_end(),
        }
    }

    /// Parses one statement.
    fn statement(&mut self) -> Parse<Statement> {
        let Some(first) = self.current() else {
            return Err(self.error("a statement"));
        };
        let start = first.start;
        let kind = match first.kind {
            TokenKind::Semicolon => {
                self.bump()?;
                StatementKind::Empty
            }
            TokenKind::Break => {
                self.bump()?;
                StatementKind::Break
            }
            TokenKind::Continue => {
                self.bump()?;
                StatementKind::Continue
            }
            TokenKind::Colon if self.peek_at(1) == Some(TokenKind::Colon) => {
                self.bump()?;
                self.bump()?;
                let name = self.expect(TokenKind::Name, "a label name")?;
                self.expect(TokenKind::Colon, "`::` to close the label")?;
                self.expect(TokenKind::Colon, "`::` to close the label")?;
                StatementKind::Label { name }
            }
            TokenKind::Goto => {
                self.bump()?;
                let label = self.expect(TokenKind::Name, "a label name")?;
                StatementKind::Goto { label }
            }
            TokenKind::Return => {
                let keyword = self.bump()?;
                let values = if ends_block(self.peek()) || self.check(TokenKind::Semicolon) {
                    Vec::new()
                } else {
                    self.expression_list()?
                };
                StatementKind::Return { keyword, values }
            }
            TokenKind::Do => {
                let keyword = self.bump()?;
                let body = self.block()?;
                self.expect_end(&keyword)?;
                StatementKind::Do { body }
            }
            TokenKind::While => {
                let keyword = self.bump()?;
                let condition = self.expression()?;
                self.expect(TokenKind::Do, "`do`")?;
                let body = self.block()?;
                self.expect_end(&keyword)?;
                StatementKind::While { condition, body }
            }
            TokenKind::Repeat => {
                let keyword = self.bump()?;
                let body = self.block()?;
                let what = format!("`until` to close `repeat` on line {}", keyword.line);
                self.expect(TokenKind::Until, &what)?;
                let condition = self.expression()?;
                StatementKind::Repeat { body, condition }
            }
            TokenKind::If => self.if_statement()?,
            TokenKind::For => self.for_statement()?,
            TokenKind::Function => self.function_statement()?,
            TokenKind::Local => self.local_statement()?,
            _ => self.expression_statement()?,
        };
        Ok(self.finish(kind, start))
    }

    /// Parses `if ... then ... {elseif ... then ...} [else ...] end`.
    fn if_statement(&mut self) -> Parse<StatementKind> {
        let opener = self.bump()?;
        let mut clauses = Vec::new();
        let mut keyword = opener;
        loop {
            let condition = self.expression()?;
            self.expect(TokenKind::Then, "`then`")?;
            let body = self.block()?;
            clauses.push(Clause {
                keyword,
                condition,
                body,
            });
            match self.eat(TokenKind::ElseIf) {
                Some(elseif) => keyword = elseif,
                None => break,
            }
        }
        let otherwise = match self.eat(TokenKind::Else) {
            Some(keyword) => Some((keyword, self.block()?)),
            None => None,
        };
        let end = self.expect_end(&opener)?;
        Ok(StatementKind::If {
            clauses,
            otherwise,
            end,
        })
    }

    /// Parses a numeric or generic `for` loop.
    fn for_statement(&mut self) -> Parse<StatementKind> {
        let keyword = self.bump()?;
        let first = self.expect(TokenKind::Name, "a loop variable name")?;
        if self.eat(TokenKind::Assign).is_some() {
            let start = self.expression()?;
            self.expect(TokenKind::Comma, "`,`")?;
            let limit = self.expression()?;
            let step = match self.eat(TokenKind::Comma) {
                Some(_) => Some(self.expression()?),
                None => None,
            };
            self.expect(TokenKind::Do, "`do`")?;
            let body = self.block()?;
            self.expect_end(&keyword)?;
            return Ok(StatementKind::NumericFor {
                variable: first,
                start,
                limit,
                step,
                body,
            });
        }
        let mut variables = vec![first];
        while self.eat(TokenKind::Comma).is_some() {
            variables.push(self.expect(TokenKind::Name, "a loop variable name")?);
        }
        self.expect(TokenKind::In, "`=` or `in`")?;
        let values = self.expression_list()?;
        self.expect(TokenKind::Do, "`do`")?;
        let body = self.block()?;
        self.expect_end(&keyword)?;
        Ok(StatementKind::GenericFor {
            variables,
            values,
            body,
        })
    }

    /// Parses `function a.b:c() ... end`.
    fn function_statement(&mut self) -> Parse<StatementKind> {
        let keyword = self.bump()?;
        let mut path = vec![self.expect(TokenKind::Name, "a function name")?];
        while self.eat(TokenKind::Dot).is_some() {
            path.push(self.expect(TokenKind::Name, "a field name")?);
        }
        let method = match self.eat(TokenKind::Colon) {
            Some(_) => Some(self.expect(TokenKind::Name, "a method name")?),
            None => None,
        };
        let function = self.function_body(keyword)?;
        Ok(StatementKind::Function {
            name: FunctionName { path, method },
            function,
        })
    }

    /// Parses `local function f() ... end` or `local a, b = ...`.
    fn local_statement(&mut self) -> Parse<StatementKind> {
        let local = self.bump()?;
        if let Some(keyword) = self.eat(TokenKind::Function) {
            let name = self.expect(TokenKind::Name, "a function name")?;
            let function = self.function_body(keyword)?;
            return Ok(StatementKind::LocalFunction {
                local,
                name,
                function,
            });
        }
        let mut names = vec![self.expect(TokenKind::Name, "a variable name or `function`")?];
        while self.eat(TokenKind::Comma).is_some() {
            names.push(self.expect(TokenKind::Name, "a variable name")?);
        }
        let values = match self.eat(TokenKind::Assign) {
            Some(_) => self.expression_list()?,
            None => Vec::new(),
        };
        Ok(StatementKind::Local {
            local,
            names,
            values,
        })
    }

    /// Parses a call statement or an assignment.
    fn expression_statement(&mut self) -> Parse<StatementKind> {
        let first_token = self.current();
        if let Some(token) = first_token
            && !starts_suffixed(token.kind)
        {
            return Err(self.unexpected(token, "a statement"));
        }
        let first = self.suffixed_expression()?;
        if !matches!(self.peek(), Some(TokenKind::Assign | TokenKind::Comma)) {
            if first.is_call() {
                return Ok(StatementKind::Call(first));
            }
            return Err(match (self.current(), first_token) {
                (Some(token), _) if !ends_block(Some(token.kind)) => {
                    self.unexpected(token, "`=` or a call")
                }
                (_, Some(token)) => ParseError {
                    range: first.range.clone(),
                    message: format!(
                        "Unexpected `{}`; an expression cannot stand as a statement.",
                        self.source
                            .text
                            .get(first.range.clone())
                            .unwrap_or_else(|| self.source.text_of(&token))
                    ),
                    too_deep: false,
                },
                _ => self.error("a statement"),
            });
        }
        let mut targets = vec![first];
        while self.eat(TokenKind::Comma).is_some() {
            targets.push(self.suffixed_expression()?);
        }
        for target in &targets {
            if !matches!(
                target.kind,
                ExpressionKind::Name(_)
                    | ExpressionKind::Field { .. }
                    | ExpressionKind::Index { .. }
            ) {
                return Err(ParseError {
                    range: target.range.clone(),
                    message: "Cannot assign to this expression; expected a name, field or index."
                        .to_owned(),
                    too_deep: false,
                });
            }
        }
        let assign = self.expect(TokenKind::Assign, "`=`")?;
        let values = self.expression_list()?;
        Ok(StatementKind::Assignment {
            targets,
            assign,
            values,
        })
    }

    /// Parses the parameter list and body of a function whose `function` keyword is `keyword`.
    fn function_body(&mut self, keyword: Token) -> Parse<Function> {
        self.descend()?;
        let open = self.expect(TokenKind::LParen, "`(` to start the parameter list")?;
        let mut parameters = Vec::new();
        let mut vararg = None;
        let close = match self.eat(TokenKind::RParen) {
            Some(close) => close,
            None => {
                loop {
                    if let Some(dots) = self.eat(TokenKind::Dots) {
                        vararg = Some(dots);
                        break;
                    }
                    parameters.push(self.expect(TokenKind::Name, "a parameter name or `...`")?);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "`)` to close the parameter list")?
            }
        };
        let body = self.block()?;
        let end = self.expect_end(&keyword)?;
        self.ascend();
        Ok(Function {
            keyword,
            parameters,
            vararg,
            open,
            close,
            body,
            end,
        })
    }

    /// Parses a comma-separated list of expressions.
    fn expression_list(&mut self) -> Parse<Vec<Expression>> {
        let mut list = vec![self.expression()?];
        while self.eat(TokenKind::Comma).is_some() {
            list.push(self.expression()?);
        }
        Ok(list)
    }

    /// Parses an expression.
    fn expression(&mut self) -> Parse<Expression> {
        self.binary_expression(0)
    }

    /// Parses an expression whose binary operators bind tighter than `limit`.
    fn binary_expression(&mut self, limit: u8) -> Parse<Expression> {
        self.descend()?;
        let mut left = match self.current() {
            Some(operator) if is_unary_operator(operator.kind) => {
                self.bump()?;
                let operand = self.binary_expression(UNARY_PRECEDENCE)?;
                Expression {
                    range: operator.start..operand.range.end,
                    kind: ExpressionKind::Unary {
                        operator,
                        operand: Box::new(operand),
                    },
                }
            }
            _ => self.simple_expression()?,
        };
        while let Some(operator) = self.current() {
            let Some((left_power, right_power)) = binary_precedence(operator.kind) else {
                break;
            };
            if left_power <= limit {
                break;
            }
            self.bump()?;
            let right = self.binary_expression(right_power)?;
            left = Expression {
                range: left.range.start..right.range.end,
                kind: ExpressionKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        self.ascend();
        Ok(left)
    }

    /// Wraps `kind` into an expression starting at `start` and ending with the previous token.
    fn expression_from(&self, kind: ExpressionKind, start: usize) -> Expression {
        Expression {
            kind,
            range: start..self.last_end(),
        }
    }

    /// Parses one operand: a literal, `...`, an anonymous function, a table, markup or a
    /// suffixed expression.
    fn simple_expression(&mut self) -> Parse<Expression> {
        let Some(token) = self.current() else {
            return Err(self.error("an expression"));
        };
        let kind = match token.kind {
            TokenKind::Nil => ExpressionKind::Nil,
            TokenKind::True => ExpressionKind::True,
            TokenKind::False => ExpressionKind::False,
            TokenKind::Number => ExpressionKind::Number,
            TokenKind::String { .. } => ExpressionKind::String(token),
            TokenKind::Dots => ExpressionKind::Vararg,
            TokenKind::Function => {
                self.bump()?;
                let function = self.function_body(token)?;
                return Ok(
                    self.expression_from(ExpressionKind::Function(Box::new(function)), token.start)
                );
            }
            TokenKind::LBrace => {
                let table = self.table()?;
                return Ok(self.expression_from(ExpressionKind::Table(table), token.start));
            }
            TokenKind::Markup => return self.markup(),
            _ => return self.suffixed_expression(),
        };
        self.bump()?;
        Ok(self.expression_from(kind, token.start))
    }

    /// Parses a Lumen markup element: its markup tokens and the Lua expression of every splice
    /// they open (`{expr}`, `{...expr}`; a splice may be empty). A markup token that closes a
    /// splice belongs to the element whose splice it closes, so the element ends before one.
    fn markup(&mut self) -> Parse<Expression> {
        let mut token = self.bump()?;
        let start = token.start;
        let mut splices = Vec::new();
        loop {
            if opens_splice(self.source.text_of(&token)) {
                if !self.at_splice_end() {
                    splices.push(self.expression()?);
                }
                if !self.at_splice_end() {
                    return Err(self.error("`}` to close the splice"));
                }
            } else if self.peek() != Some(TokenKind::Markup) || self.at_splice_end() {
                return Ok(self.expression_from(ExpressionKind::Markup(splices), start));
            }
            token = self.bump()?;
        }
    }

    /// Whether the next token is markup that closes a splice.
    fn at_splice_end(&self) -> bool {
        self.current()
            .is_some_and(|token| self.source.closes_splice(&token))
    }

    /// Parses a name, parenthesised expression or opaque value followed by any number of field
    /// accesses, indexes and calls.
    fn suffixed_expression(&mut self) -> Parse<Expression> {
        let Some(token) = self.current() else {
            return Err(self.error("an expression"));
        };
        let start = token.start;
        let mut expression = match token.kind {
            TokenKind::Name => {
                self.bump()?;
                self.expression_from(ExpressionKind::Name(token), start)
            }
            TokenKind::LParen => {
                self.bump()?;
                let inner = self.expression()?;
                self.expect(TokenKind::RParen, "`)`")?;
                self.expression_from(ExpressionKind::Parenthesized(Box::new(inner)), start)
            }
            kind if starts_suffixed(kind) => {
                self.bump()?;
                self.expression_from(ExpressionKind::Opaque, start)
            }
            _ => return Err(self.unexpected(token, "an expression")),
        };
        loop {
            let kind = match self.peek() {
                Some(TokenKind::Dot) => {
                    self.bump()?;
                    let name = self.expect(TokenKind::Name, "a field name")?;
                    ExpressionKind::Field {
                        object: Box::new(expression),
                        name,
                    }
                }
                Some(TokenKind::LBracket) => {
                    self.bump()?;
                    let key = self.expression()?;
                    self.expect(TokenKind::RBracket, "`]`")?;
                    ExpressionKind::Index {
                        object: Box::new(expression),
                        key: Box::new(key),
                    }
                }
                Some(TokenKind::Colon) if self.peek_at(1) != Some(TokenKind::Colon) => {
                    self.bump()?;
                    let name = self.expect(TokenKind::Name, "a method name")?;
                    let arguments = self.call_arguments()?;
                    ExpressionKind::MethodCall {
                        object: Box::new(expression),
                        name,
                        arguments,
                    }
                }
                Some(TokenKind::LParen | TokenKind::LBrace | TokenKind::String { .. }) => {
                    let arguments = self.call_arguments()?;
                    ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    }
                }
                _ => return Ok(expression),
            };
            expression = self.expression_from(kind, start);
        }
    }

    /// Parses the arguments of a call: `(...)`, a table or a string.
    fn call_arguments(&mut self) -> Parse<Arguments> {
        let start = self.here();
        let (style, values) = match self.peek() {
            Some(TokenKind::LParen) => {
                self.bump()?;
                let values = if self.check(TokenKind::RParen) {
                    Vec::new()
                } else {
                    self.expression_list()?
                };
                self.expect(TokenKind::RParen, "`)` to close the arguments")?;
                (ArgumentStyle::Parenthesized, values)
            }
            Some(TokenKind::LBrace) => {
                let table = self.table()?;
                let value = self.expression_from(ExpressionKind::Table(table), start);
                (ArgumentStyle::Table, vec![value])
            }
            Some(TokenKind::String { .. }) => {
                let token = self.bump()?;
                let value = self.expression_from(ExpressionKind::String(token), start);
                (ArgumentStyle::String, vec![value])
            }
            _ => return Err(self.error("arguments")),
        };
        Ok(Arguments {
            style,
            values,
            range: start..self.last_end(),
        })
    }

    /// Parses a table constructor.
    fn table(&mut self) -> Parse<Table> {
        let open = self.expect(TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        loop {
            if let Some(close) = self.eat(TokenKind::RBrace) {
                return Ok(Table {
                    open,
                    fields,
                    close,
                });
            }
            let key = match self.peek() {
                Some(TokenKind::LBracket) => {
                    self.bump()?;
                    let key = self.expression()?;
                    self.expect(TokenKind::RBracket, "`]`")?;
                    self.expect(TokenKind::Assign, "`=`")?;
                    FieldKey::Bracketed(key)
                }
                Some(TokenKind::Name) if self.peek_at(1) == Some(TokenKind::Assign) => {
                    let name = self.bump()?;
                    self.bump()?;
                    FieldKey::Named(name)
                }
                _ => FieldKey::Positional,
            };
            let value = self.expression()?;
            let separator = self
                .eat(TokenKind::Comma)
                .or_else(|| self.eat(TokenKind::Semicolon));
            let done = separator.is_none();
            fields.push(TableField {
                key,
                value,
                separator,
            });
            if done {
                let what = format!("`}}` to close the table on line {}", open.line);
                let close = self.expect(TokenKind::RBrace, &what)?;
                return Ok(Table {
                    open,
                    fields,
                    close,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(src: &str) -> Block {
        parse(&Source::new("t.lua", src)).unwrap_or_else(|error| panic!("{src:?}: {error:?}"))
    }

    fn error(src: &str) -> (u32, u32, String) {
        let source = Source::new("t.lua", src);
        let error = parse(&source).expect_err("does not parse");
        let (line, col) = source.position(error.range.start);
        (line, col, error.message)
    }

    /// The source text of every statement of the top-level block.
    fn statements(src: &str) -> Vec<String> {
        parsed(src)
            .statements
            .iter()
            .map(|statement| src[statement.range.clone()].to_owned())
            .collect()
    }

    #[test]
    fn statement_ranges() {
        assert_eq!(
            statements("local a = 1; f(x)\nif a then b() end\n::top:: goto top\n"),
            vec![
                "local a = 1",
                ";",
                "f(x)",
                "if a then b() end",
                "::top::",
                "goto top"
            ]
        );
        assert_eq!(parsed("").range, 0..0);
    }

    #[test]
    fn expressions_follow_lua_precedence() {
        let src = "x = a or b and c == d .. e .. f + g * -h ^ i\n";
        let block = parsed(src);
        let StatementKind::Assignment { values, .. } = &block.statements[0].kind else {
            panic!("assignment");
        };
        fn shape(src: &str, expression: &Expression) -> String {
            match &expression.kind {
                ExpressionKind::Binary {
                    operator,
                    left,
                    right,
                } => format!(
                    "({} {} {})",
                    shape(src, left),
                    &src[operator.range()],
                    shape(src, right)
                ),
                ExpressionKind::Unary { operator, operand } => {
                    format!("({}{})", &src[operator.range()], shape(src, operand))
                }
                _ => src[expression.range.clone()].to_owned(),
            }
        }
        assert_eq!(
            shape(src, &values[0]),
            "(a or (b and (c == (d .. (e .. (f + (g * (-(h ^ i)))))))))"
        );
    }

    #[test]
    fn glua_syntax_parses() {
        parsed("if !a && a != 2 || not b then continue end\n// c\n/* d */\n");
        parsed("for i = 1, 10, 2 do end\nfor k, v in pairs(t) do end\nrepeat until x\n");
        parsed("local t = { 1, a = 2, [3] = 4; 5, }\nf{} f'x' f[[y]] a.b:c(1)['d'].e()\n");
        parsed("return\n");
        parsed("do return end\nwhile x do break end\n");
        parsed("local f = function(a, ...) return ... end\n");
        parsed("return;\n");
    }

    #[test]
    fn calls_and_targets() {
        let block = parsed("a.b, c[1] = f()\nobj:m 'x'\n");
        assert!(matches!(
            block.statements[0].kind,
            StatementKind::Assignment { .. }
        ));
        let StatementKind::Call(call) = &block.statements[1].kind else {
            panic!("call");
        };
        let ExpressionKind::MethodCall { arguments, .. } = &call.kind else {
            panic!("method call");
        };
        assert_eq!(arguments.style, ArgumentStyle::String);
    }

    #[test]
    fn helpers() {
        let src = "util.AddNetworkString('a')\nrequire [[b]]\nf()()\n";
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let calls: Vec<(String, Option<&str>)> = block
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StatementKind::Call(call) => call.as_call(&source),
                _ => None,
            })
            .map(|(name, arguments)| {
                (
                    name,
                    arguments
                        .values
                        .first()
                        .and_then(|value| value.string_contents(&source)),
                )
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                ("util.AddNetworkString".to_owned(), Some("a")),
                ("require".to_owned(), Some("b"))
            ]
        );
    }

    #[test]
    fn walks_in_source_order() {
        struct Names(Vec<String>, String);
        impl<'t> Visitor<'t> for Names {
            fn expression(&mut self, expression: &'t Expression) {
                if let ExpressionKind::Name(token) = expression.kind {
                    self.0.push(self.1[token.range()].to_owned());
                }
            }
        }
        let src = "local a = b\nif c then d() elseif e then else f() end\n\
                   repeat g() until h\nlocal x = function() return i end\n";
        let mut names = Names(Vec::new(), src.to_owned());
        walk(&parsed(src), &mut names);
        assert_eq!(names.0, vec!["b", "c", "d", "e", "f", "g", "h", "i"]);
    }

    #[test]
    fn reports_malformed_code() {
        assert_eq!(
            error("if x y() end\n"),
            (1, 6, "Unexpected `y`; expected `then`.".to_owned())
        );
        assert_eq!(
            error("local = 1\n"),
            (
                1,
                7,
                "Unexpected `=`; expected a variable name or `function`.".to_owned()
            )
        );
        assert_eq!(
            error("x\n"),
            (
                1,
                1,
                "Unexpected `x`; an expression cannot stand as a statement.".to_owned()
            )
        );
        assert_eq!(
            error("x y\n"),
            (1, 3, "Unexpected `y`; expected `=` or a call.".to_owned())
        );
        assert_eq!(
            error("f() = 1\n"),
            (
                1,
                1,
                "Cannot assign to this expression; expected a name, field or index.".to_owned()
            )
        );
        assert_eq!(
            error("return 1\nx = 2\n"),
            (
                2,
                1,
                "Unexpected `x`; expected the end of the block after `return`.".to_owned()
            )
        );
        assert_eq!(
            error("function f()\n"),
            (
                2,
                1,
                "Unexpected end of file; expected `end` to close `function` on line 1.".to_owned()
            )
        );
        assert_eq!(
            error("t = { a = 1 b = 2 }\n"),
            (
                1,
                13,
                "Unexpected `b`; expected `}` to close the table on line 1.".to_owned()
            )
        );
        assert_eq!(
            error("x = 1 +\n"),
            (
                2,
                1,
                "Unexpected end of file; expected an expression.".to_owned()
            )
        );
    }

    #[test]
    fn deep_nesting_is_not_malformed() {
        let deep = format!("x = {}1{}\n", "(".repeat(500), ")".repeat(500));
        let error = parse(&Source::new("t.lua", deep)).expect_err("too deep");
        assert!(error.too_deep);
    }

    #[test]
    fn lumen_markup_is_an_operand() {
        let src = "return function(props)\n  return <view a={props.a} {...props.c}>{props.b}{}</view>\nend\n";
        let source = Source::new("t.lumen", src);
        let block = parse(&source).unwrap_or_else(|error| panic!("{error:?}"));
        struct Splices(usize);
        impl<'t> Visitor<'t> for Splices {
            fn expression(&mut self, expression: &'t Expression) {
                if let ExpressionKind::Markup(splices) = &expression.kind {
                    self.0 += splices.len();
                }
            }
        }
        let mut splices = Splices(0);
        walk(&block, &mut splices);
        assert_eq!(splices.0, 3);
    }

    #[test]
    fn unknown_tokens_are_opaque_values() {
        parsed("local a = @ + b\n");
    }

    #[test]
    fn string_contents() {
        let src = "f('a\\'b', \"c\", [==[\nd]==], [[]])\n";
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let StatementKind::Call(call) = &block.statements[0].kind else {
            panic!("call");
        };
        let (_, arguments) = call.as_call(&source).expect("call");
        let contents: Vec<Option<&str>> = arguments
            .values
            .iter()
            .map(|value| value.string_contents(&source))
            .collect();
        assert_eq!(
            contents,
            vec![Some("a\\'b"), Some("c"), Some("d"), Some("")]
        );
    }
}
