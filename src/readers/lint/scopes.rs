//! A scope resolver over the token stream, shared by the lint readers.
//!
//! [`resolve`] walks the code tokens with a small recursive descent parser that knows just enough
//! of the GLua grammar to track blocks and statements. It binds every variable reference to the
//! declaration it resolves to and counts how each declared variable is read and written. A file
//! that does not parse yields `None`, so callers never act on a partial picture.
//!
//! Scoping follows Lua: a `local` statement's names are visible after the whole statement (the
//! right-hand side of `local x = x` sees the outer `x`), a `local function` is visible inside its
//! own body, parameters and loop variables are visible in the body, every `if` branch is its own
//! block, and the `until` condition of a `repeat` loop sees the locals of the loop body. A
//! `const` declaration declares locals like `local`, the parameters of a short function
//! (`|a, b| -> a + b`) are parameters, and a compound assignment (`x += 1`) reads and writes its
//! target.
//!
//! The component tags of a Lumen template (see [`Source::component_references`]) are reads of
//! the variable they name, resolved at the offset of the tag. The Lua spliced into markup is
//! parsed as expressions; other tokens the grammar has no place for, such as unrecognised bytes,
//! are opaque values that never declare anything.

use std::ops::Range;

use crate::lumen::opens_splice;
use crate::readers::naming::declarations::{DeclarationKind, significant_tokens};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// The UTF-8 byte order mark, which the lexer keeps as an `Unknown` token.
const BOM: &str = "\u{feff}";

/// Deepest nesting of blocks and expressions the resolver follows before giving up on a file.
const MAX_DEPTH: usize = 200;

/// A declared variable and how the code uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// The variable name.
    pub name: String,
    /// How the variable is introduced.
    pub kind: DeclarationKind,
    /// The declaring `Name` token; for the implicit `self` of a method, the method name.
    pub token: Token,
    /// Whether this is the implicit `self` parameter of a `function a:b()` method.
    pub implicit: bool,
    /// Bytes from the start of the declaring construct (the `local`, `for` or `(` of the
    /// parameter list) to the end of the block the variable lives in.
    pub scope: Range<usize>,
    /// How many times the variable is read.
    pub reads: usize,
    /// The `Name` tokens that assign to the variable after its declaration: assignment targets
    /// and `function name()` definitions.
    pub writes: Vec<Token>,
    /// For a parameter, the `function` keyword of its function.
    pub function: Option<Token>,
    /// The variable this one shadows: the one its name resolved to where it was declared (an
    /// earlier declaration in the same scope or one in an enclosing scope). Always `None` for
    /// the implicit `self`.
    pub shadows: Option<usize>,
}

/// A variable name read or assigned in the code, with the variable it resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    /// The `Name` token.
    pub token: Token,
    /// Index into [`Resolution::variables`] of the local it resolves to; `None` for a global.
    pub variable: Option<usize>,
    /// Whether the name is assigned rather than read.
    pub write: bool,
}

/// The outcome of resolving a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'a> {
    /// Every declared variable, in declaration order.
    pub variables: Vec<Variable>,
    /// Every variable name occurrence outside field position, as `(byte offset, name)`:
    /// declarations, reads, writes and component tags, whether or not they resolve to a local.
    pub names: Vec<(usize, &'a str)>,
    /// Every read and assignment of a variable name (component tags excluded), sorted by offset.
    pub references: Vec<Reference>,
}

impl Resolution<'_> {
    /// The reference whose name token starts at byte `offset`.
    pub fn reference_at(&self, offset: usize) -> Option<&Reference> {
        let index = self
            .references
            .partition_point(|reference| reference.token.start < offset);
        self.references
            .get(index)
            .filter(|reference| reference.token.start == offset)
    }

    /// The variable a read of `name` at byte `offset` would resolve to: the latest declaration
    /// of `name` that is visible there. A local is treated as visible from the end of its name,
    /// so the right-hand side of its own `local` statement counts as shadowed.
    pub fn visible_at(&self, name: &str, offset: usize) -> Option<usize> {
        self.variables
            .iter()
            .enumerate()
            .filter(|(_, variable)| {
                variable.name == name
                    && variable.token.end <= offset
                    && variable.scope.start <= offset
                    && offset < variable.scope.end
            })
            .map(|(id, _)| id)
            .next_back()
    }

    /// Whether `name` is declared or referenced anywhere inside `range`.
    pub fn mentions(&self, name: &str, range: &Range<usize>) -> bool {
        self.names
            .iter()
            .any(|(offset, mention)| *mention == name && range.contains(offset))
    }
}

/// Resolves the variables of `source`.
///
/// Returns `None` when the code does not parse (a missing `end`, a stray keyword, nesting deeper
/// than the resolver follows, ...).
pub fn resolve(source: &Source) -> Option<Resolution<'_>> {
    let tokens: Vec<Token> = significant_tokens(source)
        .into_iter()
        .filter(|token| !(token.kind == TokenKind::Unknown && source.text_of(token) == BOM))
        .collect();
    let mut resolver = Resolver {
        source,
        tokens,
        pos: 0,
        components: source.component_references(),
        next_component: 0,
        variables: Vec::new(),
        names: Vec::new(),
        references: Vec::new(),
        scopes: Vec::new(),
        depth: 0,
    };
    resolver.chunk().ok()?;
    let mut references = resolver.references;
    references.sort_by_key(|reference| reference.token.start);
    Some(Resolution {
        variables: resolver.variables,
        names: resolver.names,
        references,
    })
}

/// Marker for code the resolver cannot parse.
#[derive(Debug)]
struct Unparsable;

/// The result of a parsing step.
type Parse<T = ()> = Result<T, Unparsable>;

/// Parser state: the tokens, the cursor, the open scopes and what has been found so far.
struct Resolver<'a> {
    source: &'a Source,
    tokens: Vec<Token>,
    pos: usize,
    /// The component tags of a Lumen template, as `(name, byte offset)` in source order.
    components: Vec<(&'a str, usize)>,
    /// How many of `components` have been resolved.
    next_component: usize,
    variables: Vec<Variable>,
    names: Vec<(usize, &'a str)>,
    references: Vec<Reference>,
    scopes: Vec<Vec<usize>>,
    depth: usize,
}

/// Whether `kind` has a fixed role in the grammar: keywords, operators, punctuation, whitespace,
/// comments and the end of the input. Every other kind (names, literals, markup and unrecognised
/// bytes) can stand for a value.
fn is_structural(kind: TokenKind) -> bool {
    kind.is_keyword()
        || matches!(
            kind,
            TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Caret
                | TokenKind::Hash
                | TokenKind::Eq
                | TokenKind::Ne
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
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
                | TokenKind::Concat
                | TokenKind::Dots
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::AndAnd
                | TokenKind::OrOr
                | TokenKind::Ampersand
                | TokenKind::Pipe
                | TokenKind::Bar
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::Sar
                | TokenKind::Question
                | TokenKind::TernaryColon
                | TokenKind::SafeNav
                | TokenKind::Coalesce
                | TokenKind::Arrow
                | TokenKind::CompoundAssign
                | TokenKind::Space
                | TokenKind::Tab
                | TokenKind::Newline
                | TokenKind::Comment { .. }
                | TokenKind::Eof
        )
}

/// Whether `kind` is a binary operator.
fn is_binary_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::Concat
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Ampersand
            | TokenKind::Pipe
            | TokenKind::Tilde
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Sar
            | TokenKind::Coalesce
    )
}

/// Whether `kind` is a unary operator.
fn is_unary_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Not | TokenKind::Minus | TokenKind::Hash | TokenKind::Bang | TokenKind::Tilde
    )
}

impl<'a> Resolver<'a> {
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

    /// Consumes the token at the cursor, first resolving the component tags before its end.
    fn bump(&mut self) -> Parse<Token> {
        let token = *self.tokens.get(self.pos).ok_or(Unparsable)?;
        self.resolve_components(token.end);
        self.pos += 1;
        Ok(token)
    }

    /// Consumes the token at the cursor when it is of `kind`.
    fn eat(&mut self, kind: TokenKind) -> Parse<bool> {
        if self.check(kind) {
            self.bump()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Consumes a token of `kind`, failing when the cursor is on anything else.
    fn expect(&mut self, kind: TokenKind) -> Parse<Token> {
        if self.check(kind) {
            return self.bump();
        }
        Err(Unparsable)
    }

    /// Byte offset of the token at the cursor, or the end of the text.
    fn here(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map_or(self.source.text.len(), |token| token.start)
    }

    /// Text of `token`.
    fn text(&self, token: &Token) -> &'a str {
        self.source.text_of(token)
    }

    /// Counts the component tags located before byte `limit` as reads.
    fn resolve_components(&mut self, limit: usize) {
        while let Some(&(name, offset)) = self.components.get(self.next_component) {
            if offset >= limit {
                break;
            }
            self.next_component += 1;
            self.names.push((offset, name));
            if let Some(id) = self.lookup(name) {
                self.variables[id].reads += 1;
            }
        }
    }

    /// Enters one more level of nesting, failing past [`MAX_DEPTH`].
    fn descend(&mut self) -> Parse {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Unparsable);
        }
        Ok(())
    }

    /// Leaves a level of nesting entered with [`Resolver::descend`].
    fn ascend(&mut self) {
        self.depth -= 1;
    }

    /// Opens a new innermost scope.
    fn open_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    /// Closes the innermost scope at the cursor: the component tags before the cursor still see
    /// it, and its variables' scopes end there.
    fn close_scope(&mut self) {
        let end = self.here();
        self.resolve_components(end);
        for id in self.scopes.pop().unwrap_or_default() {
            self.variables[id].scope.end = end;
        }
    }

    /// Declares the variable named by `token` in the innermost scope.
    fn declare(
        &mut self,
        token: Token,
        kind: DeclarationKind,
        start: usize,
        function: Option<Token>,
    ) {
        let name = self.text(&token);
        self.names.push((token.start, name));
        let shadows = self.lookup(name);
        self.push_variable(Variable {
            name: name.to_owned(),
            kind,
            token,
            implicit: false,
            scope: start..start,
            reads: 0,
            writes: Vec::new(),
            function,
            shadows,
        });
    }

    /// Adds `variable` to the innermost scope.
    fn push_variable(&mut self, variable: Variable) {
        let id = self.variables.len();
        self.variables.push(variable);
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(id);
        }
    }

    /// The variable `name` resolves to from the cursor: the latest declaration in the innermost
    /// scope that has one.
    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .copied()
            .find(|&id| self.variables[id].name == name)
    }

    /// Records a read of the variable named by `token`.
    fn read(&mut self, token: Token) {
        let name = self.text(&token);
        self.names.push((token.start, name));
        let variable = self.lookup(name);
        if let Some(id) = variable {
            self.variables[id].reads += 1;
        }
        self.references.push(Reference {
            token,
            variable,
            write: false,
        });
    }

    /// Records an assignment to the variable named by `token`.
    fn write(&mut self, token: Token) {
        let name = self.text(&token);
        self.names.push((token.start, name));
        let variable = self.lookup(name);
        if let Some(id) = variable {
            self.variables[id].writes.push(token);
        }
        self.references.push(Reference {
            token,
            variable,
            write: true,
        });
    }

    /// Parses the whole file as one block.
    fn chunk(&mut self) -> Parse {
        self.open_scope();
        self.block()?;
        if self.pos < self.tokens.len() {
            return Err(Unparsable);
        }
        self.close_scope();
        Ok(())
    }

    /// Parses statements up to a block terminator (`end`, `else`, `elseif`, `until` or the end
    /// of the file), which is left for the caller.
    fn block(&mut self) -> Parse {
        self.descend()?;
        while let Some(kind) = self.peek() {
            if matches!(
                kind,
                TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until
            ) {
                break;
            }
            self.statement()?;
        }
        self.ascend();
        Ok(())
    }

    /// Parses `block` inside a scope of its own.
    fn scoped_block(&mut self) -> Parse {
        self.open_scope();
        self.block()?;
        self.close_scope();
        Ok(())
    }

    /// Parses one statement.
    fn statement(&mut self) -> Parse {
        let Some(kind) = self.peek() else {
            return Err(Unparsable);
        };
        match kind {
            TokenKind::Semicolon | TokenKind::Break | TokenKind::Continue => {
                self.bump()?;
            }
            TokenKind::Colon if self.peek_at(1) == Some(TokenKind::Colon) => {
                self.bump()?;
                self.bump()?;
                self.bump()?;
                self.expect(TokenKind::Colon)?;
                self.expect(TokenKind::Colon)?;
            }
            TokenKind::Goto => {
                self.bump()?;
                self.bump()?;
            }
            TokenKind::Return => {
                self.bump()?;
                if !matches!(
                    self.peek(),
                    None | Some(
                        TokenKind::End
                            | TokenKind::Else
                            | TokenKind::ElseIf
                            | TokenKind::Until
                            | TokenKind::Semicolon
                    )
                ) {
                    self.expression_list()?;
                }
            }
            TokenKind::Do => {
                self.bump()?;
                self.scoped_block()?;
                self.expect(TokenKind::End)?;
            }
            TokenKind::While => {
                self.bump()?;
                self.expression()?;
                self.expect(TokenKind::Do)?;
                self.scoped_block()?;
                self.expect(TokenKind::End)?;
            }
            TokenKind::Repeat => {
                self.bump()?;
                self.open_scope();
                self.block()?;
                self.expect(TokenKind::Until)?;
                self.expression()?;
                self.close_scope();
            }
            TokenKind::If => self.if_statement()?,
            TokenKind::For => self.for_statement()?,
            TokenKind::Function => self.function_statement()?,
            TokenKind::Local | TokenKind::Const => self.local_statement()?,
            _ => self.expression_statement()?,
        }
        Ok(())
    }

    /// Parses `if ... then ... {elseif ... then ...} [else ...] end`.
    fn if_statement(&mut self) -> Parse {
        self.bump()?;
        self.expression()?;
        self.expect(TokenKind::Then)?;
        self.scoped_block()?;
        loop {
            if self.eat(TokenKind::ElseIf)? {
                self.expression()?;
                self.expect(TokenKind::Then)?;
                self.scoped_block()?;
            } else if self.eat(TokenKind::Else)? {
                self.scoped_block()?;
            } else {
                self.expect(TokenKind::End)?;
                return Ok(());
            }
        }
    }

    /// Parses a numeric or generic `for` loop.
    fn for_statement(&mut self) -> Parse {
        let keyword = self.bump()?;
        let mut names = vec![self.expect(TokenKind::Name)?];
        if self.eat(TokenKind::Assign)? {
            self.expression()?;
            self.expect(TokenKind::Comma)?;
            self.expression()?;
            if self.eat(TokenKind::Comma)? {
                self.expression()?;
            }
        } else {
            while self.eat(TokenKind::Comma)? {
                names.push(self.expect(TokenKind::Name)?);
            }
            self.expect(TokenKind::In)?;
            self.expression_list()?;
        }
        self.expect(TokenKind::Do)?;
        self.open_scope();
        for name in names {
            self.declare(name, DeclarationKind::ForVariable, keyword.start, None);
        }
        self.block()?;
        self.close_scope();
        self.expect(TokenKind::End)?;
        Ok(())
    }

    /// Parses `function a.b:c() ... end`; `a` is read, or assigned when it is the whole name.
    fn function_statement(&mut self) -> Parse {
        let keyword = self.bump()?;
        let base = self.expect(TokenKind::Name)?;
        let mut method = None;
        if matches!(self.peek(), Some(TokenKind::Dot | TokenKind::Colon)) {
            self.read(base);
            while self.eat(TokenKind::Dot)? {
                self.expect(TokenKind::Name)?;
            }
            if self.eat(TokenKind::Colon)? {
                method = Some(self.expect(TokenKind::Name)?);
            }
        } else {
            self.write(base);
        }
        self.function_body(keyword, method)
    }

    /// Parses `local function f() ... end` or `local a, b = ...`, or the same with `const`.
    fn local_statement(&mut self) -> Parse {
        let local = self.bump()?;
        if self.check(TokenKind::Function) {
            let keyword = self.bump()?;
            let name = self.expect(TokenKind::Name)?;
            self.declare(name, DeclarationKind::LocalFunction, local.start, None);
            return self.function_body(keyword, None);
        }
        let mut names = vec![self.expect(TokenKind::Name)?];
        while self.eat(TokenKind::Comma)? {
            names.push(self.expect(TokenKind::Name)?);
        }
        if self.eat(TokenKind::Assign)? {
            self.expression_list()?;
        }
        for name in names {
            self.declare(name, DeclarationKind::Local, local.start, None);
        }
        Ok(())
    }

    /// Parses a call statement or an assignment; a bare name on its own is unparsable. A
    /// compound assignment reads and writes its target.
    fn expression_statement(&mut self) -> Parse {
        let first = self.suffixed_expression()?;
        if self.eat(TokenKind::CompoundAssign)? {
            self.expression()?;
            if let Some(name) = first {
                self.write(name);
                self.read(name);
            }
            return Ok(());
        }
        if !matches!(self.peek(), Some(TokenKind::Assign | TokenKind::Comma)) {
            return match first {
                Some(_) => Err(Unparsable),
                None => Ok(()),
            };
        }
        let mut targets = vec![first];
        while self.eat(TokenKind::Comma)? {
            targets.push(self.suffixed_expression()?);
        }
        self.expect(TokenKind::Assign)?;
        self.expression_list()?;
        for name in targets.into_iter().flatten() {
            self.write(name);
        }
        Ok(())
    }

    /// Parses the parameter list and body of a function whose `function` keyword is `keyword`;
    /// `method` is the method name of a `function a:b()` definition.
    fn function_body(&mut self, keyword: Token, method: Option<Token>) -> Parse {
        self.descend()?;
        self.open_scope();
        let open = self.expect(TokenKind::LParen)?;
        if let Some(method) = method {
            self.push_variable(Variable {
                name: "self".to_owned(),
                kind: DeclarationKind::Parameter,
                token: method,
                implicit: true,
                scope: open.start..open.start,
                reads: 0,
                writes: Vec::new(),
                function: Some(keyword),
                shadows: None,
            });
        }
        if !self.eat(TokenKind::RParen)? {
            loop {
                if self.eat(TokenKind::Dots)? {
                    break;
                }
                let name = self.expect(TokenKind::Name)?;
                self.declare(name, DeclarationKind::Parameter, open.start, Some(keyword));
                if !self.eat(TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(TokenKind::RParen)?;
        }
        self.block()?;
        self.close_scope();
        self.expect(TokenKind::End)?;
        self.ascend();
        Ok(())
    }

    /// Parses a comma-separated list of expressions.
    fn expression_list(&mut self) -> Parse {
        self.expression()?;
        while self.eat(TokenKind::Comma)? {
            self.expression()?;
        }
        Ok(())
    }

    /// Parses an expression: operands joined by binary operators, each with any unary prefixes,
    /// optionally followed by the `? a : b` of a ternary.
    fn expression(&mut self) -> Parse {
        self.descend()?;
        loop {
            while self.peek().is_some_and(is_unary_operator) {
                self.bump()?;
            }
            self.simple_expression()?;
            if !self.peek().is_some_and(is_binary_operator) {
                break;
            }
            self.bump()?;
        }
        if self.eat(TokenKind::Question)? {
            self.expression()?;
            self.expect(TokenKind::TernaryColon)?;
            self.expression()?;
        }
        self.ascend();
        Ok(())
    }

    /// Parses one operand: a literal, `...`, an anonymous function, a table or a suffixed
    /// expression.
    fn simple_expression(&mut self) -> Parse {
        match self.peek() {
            Some(
                TokenKind::Nil
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Number
                | TokenKind::String { .. }
                | TokenKind::Dots,
            ) => {
                self.bump()?;
            }
            Some(TokenKind::Function) => {
                let keyword = self.bump()?;
                self.function_body(keyword, None)?;
            }
            Some(TokenKind::LBrace) => self.table()?,
            Some(TokenKind::Markup) => self.markup()?,
            Some(TokenKind::Bar) => self.short_function()?,
            Some(TokenKind::Name) if self.peek_at(1) == Some(TokenKind::Arrow) => {
                self.short_function()?;
            }
            _ => {
                if let Some(name) = self.suffixed_expression()? {
                    self.read(name);
                }
            }
        }
        Ok(())
    }

    /// Parses a short function (`|a, b| -> ...`, `|| -> ...`, `a -> ...`): its parameters, declared
    /// for its body, and an expression or `do ... end` body.
    fn short_function(&mut self) -> Parse {
        self.descend()?;
        self.open_scope();
        let first = self.bump()?;
        if first.kind == TokenKind::Name {
            self.declare(first, DeclarationKind::Parameter, first.start, Some(first));
        } else {
            while !self.check(TokenKind::Bar) {
                if self.eat(TokenKind::Dots)? {
                    break;
                }
                let name = self.expect(TokenKind::Name)?;
                self.declare(name, DeclarationKind::Parameter, first.start, Some(first));
                if !self.eat(TokenKind::Comma)? {
                    break;
                }
            }
            self.expect(TokenKind::Bar)?;
        }
        self.expect(TokenKind::Arrow)?;
        if self.eat(TokenKind::Do)? {
            self.block()?;
            self.close_scope();
            self.expect(TokenKind::End)?;
        } else {
            self.expression()?;
            self.close_scope();
        }
        self.ascend();
        Ok(())
    }

    /// Parses a Lumen markup element: its markup tokens and the Lua expression of every splice
    /// they open (`{expr}`, `{...expr}`; a splice may be empty). A markup token that closes a
    /// splice belongs to the element whose splice it closes, so the element ends before one.
    fn markup(&mut self) -> Parse {
        let mut token = self.bump()?;
        loop {
            if opens_splice(self.source.text_of(&token)) {
                if !self.at_splice_end() {
                    self.expression()?;
                }
                if !self.at_splice_end() {
                    return Err(Unparsable);
                }
            } else if self.peek() != Some(TokenKind::Markup) || self.at_splice_end() {
                return Ok(());
            }
            token = self.bump()?;
        }
    }

    /// Whether the next token is markup that closes a splice.
    fn at_splice_end(&self) -> bool {
        self.tokens
            .get(self.pos)
            .is_some_and(|token| self.source.closes_splice(token))
    }

    /// Parses a name, parenthesised expression or opaque value followed by any number of field
    /// accesses, indexes and calls. Returns the name when the expression is a bare name, whose
    /// use (read or write) the caller decides; a name followed by a suffix is read here.
    fn suffixed_expression(&mut self) -> Parse<Option<Token>> {
        let mut bare = match self.peek() {
            Some(TokenKind::Name) => Some(self.bump()?),
            Some(TokenKind::LParen) => {
                self.bump()?;
                self.expression()?;
                self.expect(TokenKind::RParen)?;
                None
            }
            Some(kind)
                if !is_structural(kind) && !kind.is_string() && kind != TokenKind::Number =>
            {
                self.bump()?;
                None
            }
            _ => return Err(Unparsable),
        };
        loop {
            match self.peek() {
                Some(TokenKind::SafeNav) => {
                    self.read_bare(&mut bare);
                    self.bump()?;
                    match self.peek() {
                        Some(TokenKind::Name) => {
                            self.bump()?;
                        }
                        Some(TokenKind::LBracket) => {
                            self.bump()?;
                            self.expression()?;
                            self.expect(TokenKind::RBracket)?;
                        }
                        Some(TokenKind::Colon) => {
                            self.bump()?;
                            self.expect(TokenKind::Name)?;
                            self.call_arguments()?;
                        }
                        _ => self.call_arguments()?,
                    }
                }
                Some(TokenKind::Dot) => {
                    self.read_bare(&mut bare);
                    self.bump()?;
                    self.expect(TokenKind::Name)?;
                }
                Some(TokenKind::LBracket) => {
                    self.read_bare(&mut bare);
                    self.bump()?;
                    self.expression()?;
                    self.expect(TokenKind::RBracket)?;
                }
                Some(TokenKind::Colon) if self.peek_at(1) != Some(TokenKind::Colon) => {
                    self.read_bare(&mut bare);
                    self.bump()?;
                    self.expect(TokenKind::Name)?;
                    self.eat(TokenKind::SafeNav)?;
                    self.call_arguments()?;
                }
                Some(TokenKind::LParen | TokenKind::LBrace | TokenKind::String { .. }) => {
                    self.read_bare(&mut bare);
                    self.call_arguments()?;
                }
                _ => return Ok(bare),
            }
        }
    }

    /// Records the bare name of a suffixed expression as read, once a suffix follows it.
    fn read_bare(&mut self, bare: &mut Option<Token>) {
        if let Some(name) = bare.take() {
            self.read(name);
        }
    }

    /// Parses the arguments of a call: `(...)`, a table or a string.
    fn call_arguments(&mut self) -> Parse {
        match self.peek() {
            Some(TokenKind::LParen) => {
                self.bump()?;
                if !self.eat(TokenKind::RParen)? {
                    self.expression_list()?;
                    self.expect(TokenKind::RParen)?;
                }
            }
            Some(TokenKind::LBrace) => self.table()?,
            Some(TokenKind::String { .. }) => {
                self.bump()?;
            }
            _ => return Err(Unparsable),
        }
        Ok(())
    }

    /// Parses a table constructor; `name = value` keys are not references, `[key]` keys are.
    fn table(&mut self) -> Parse {
        self.expect(TokenKind::LBrace)?;
        loop {
            if self.eat(TokenKind::RBrace)? {
                return Ok(());
            }
            match self.peek() {
                Some(TokenKind::LBracket) => {
                    self.bump()?;
                    self.expression()?;
                    self.expect(TokenKind::RBracket)?;
                    self.expect(TokenKind::Assign)?;
                    self.expression()?;
                }
                Some(TokenKind::Name) if self.peek_at(1) == Some(TokenKind::Assign) => {
                    self.bump()?;
                    self.bump()?;
                    self.expression()?;
                }
                _ => self.expression()?,
            }
            if !self.eat(TokenKind::Comma)? && !self.eat(TokenKind::Semicolon)? {
                self.expect(TokenKind::RBrace)?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(name, reads, writes)` of every explicit variable of `src`, in declaration order.
    fn usage(src: &str) -> Vec<(String, usize, usize)> {
        let source = Source::new("t.lua", src);
        resolve(&source)
            .expect("parses")
            .variables
            .into_iter()
            .filter(|variable| !variable.implicit)
            .map(|variable| (variable.name, variable.reads, variable.writes.len()))
            .collect()
    }

    fn entry(name: &str, reads: usize, writes: usize) -> (String, usize, usize) {
        (name.to_owned(), reads, writes)
    }

    #[test]
    fn local_right_hand_side_sees_the_outer_variable() {
        assert_eq!(
            usage("local x = 1\nlocal x = x\n"),
            vec![entry("x", 1, 0), entry("x", 0, 0)]
        );
    }

    #[test]
    fn local_function_sees_itself_but_local_function_value_does_not() {
        assert_eq!(
            usage("local function f() return f() end\n"),
            vec![entry("f", 1, 0)]
        );
        assert_eq!(
            usage("local f = function() return f() end\n"),
            vec![entry("f", 0, 0)]
        );
    }

    #[test]
    fn repeat_condition_sees_the_body() {
        assert_eq!(
            usage("repeat local done = true until done\n"),
            vec![entry("done", 1, 0)]
        );
    }

    #[test]
    fn branches_are_separate_scopes() {
        assert_eq!(
            usage("local a\nif x then local a = 1 elseif y then a = 2 else print(a) end\n"),
            vec![entry("a", 1, 1), entry("a", 0, 0)]
        );
    }

    #[test]
    fn writes_and_reads_are_told_apart() {
        assert_eq!(
            usage("local a, b, c, d\na, b.x, c[1] = 1, 2, 3\nd = d + 1\n"),
            vec![
                entry("a", 0, 1),
                entry("b", 1, 0),
                entry("c", 1, 0),
                entry("d", 1, 1)
            ]
        );
        assert_eq!(
            usage("local f\nfunction f() end\nlocal g\nfunction g.h() end\n"),
            vec![entry("f", 0, 1), entry("g", 1, 0)]
        );
    }

    #[test]
    fn fields_keys_and_labels_are_not_references() {
        assert_eq!(
            usage("local x, y, z\nprint(a.x, a:y(), { z = 1 })\n::x:: goto y\n"),
            vec![entry("x", 0, 0), entry("y", 0, 0), entry("z", 0, 0)]
        );
        assert_eq!(
            usage("local k\nprint({ [k] = 1 })\n"),
            vec![entry("k", 1, 0)]
        );
    }

    #[test]
    fn methods_declare_an_implicit_self() {
        let source = Source::new("t.lua", "local self = 1\nfunction a:b() return self end\n");
        let resolution = resolve(&source).expect("parses");
        let reads: Vec<(bool, usize)> = resolution
            .variables
            .iter()
            .map(|variable| (variable.implicit, variable.reads))
            .collect();
        assert_eq!(reads, vec![(false, 0), (true, 1)]);
    }

    #[test]
    fn scopes_cover_the_declaring_construct_and_block() {
        let src = "do local a = 1 end\nfunction f(p) end\n";
        let source = Source::new("t.lua", src);
        let resolution = resolve(&source).expect("parses");
        let scopes: Vec<&str> = resolution
            .variables
            .iter()
            .map(|variable| &src[variable.scope.clone()])
            .collect();
        assert_eq!(scopes, vec!["local a = 1 ", "(p) "]);
    }

    #[test]
    fn component_tags_resolve_at_their_offset() {
        let src = "local Card = 1\nlocal Row = 2\ndo\n  local Card = <Card/>\n  \
                   print(<Card/>)\nend\nreturn <Card><Row/></Card>\n";
        let source = Source::new("t.lumen", src);
        let resolution = resolve(&source).expect("parses");
        let reads: Vec<(&str, usize)> = resolution
            .variables
            .iter()
            .map(|variable| (variable.name.as_str(), variable.reads))
            .collect();
        assert_eq!(reads, vec![("Card", 2), ("Row", 1), ("Card", 1)]);
        let tag = src.find("Card/>").expect("tag");
        assert!(resolution.mentions("Card", &(tag..tag + 1)));
    }

    #[test]
    fn declarations_record_what_they_shadow() {
        let src = "local a = 1\nlocal a = a\nfunction f(a, b) for b = 1, 2 do end end\n";
        let source = Source::new("t.lua", src);
        let resolution = resolve(&source).expect("parses");
        let shadows: Vec<(&str, Option<usize>)> = resolution
            .variables
            .iter()
            .map(|variable| (variable.name.as_str(), variable.shadows))
            .collect();
        assert_eq!(
            shadows,
            vec![
                ("a", None),
                ("a", Some(0)),
                ("a", Some(1)),
                ("b", None),
                ("b", Some(3))
            ]
        );
    }

    #[test]
    fn references_are_bound_and_sorted() {
        let src = "local t\nx, t = t, y\n";
        let source = Source::new("t.lua", src);
        let resolution = resolve(&source).expect("parses");
        let references: Vec<(&str, Option<usize>, bool)> = resolution
            .references
            .iter()
            .map(|reference| {
                (
                    &src[reference.token.range()],
                    reference.variable,
                    reference.write,
                )
            })
            .collect();
        assert_eq!(
            references,
            vec![
                ("x", None, true),
                ("t", Some(0), true),
                ("t", Some(0), false),
                ("y", None, false)
            ]
        );
        let offset = src.find("y").expect("y");
        assert_eq!(
            resolution
                .reference_at(offset)
                .map(|reference| reference.variable),
            Some(None)
        );
        assert!(resolution.reference_at(offset + 1).is_none());
    }

    #[test]
    fn visibility_follows_scopes() {
        let src = "local a = 1\ndo local a = 2 print(a) end\nprint(a)\n";
        let source = Source::new("t.lua", src);
        let resolution = resolve(&source).expect("parses");
        let inner = src.find("print(a) end").expect("inner") + 6;
        let outer = src.rfind("print(a)").expect("outer") + 6;
        assert_eq!(resolution.visible_at("a", inner), Some(1));
        assert_eq!(resolution.visible_at("a", outer), Some(0));
        assert_eq!(resolution.visible_at("a", 0), None);
        assert_eq!(resolution.visible_at("b", outer), None);
    }

    #[test]
    fn unknown_tokens_are_opaque_values() {
        assert_eq!(usage("local a = @ + b\n"), vec![entry("a", 0, 0)]);
    }

    #[test]
    fn unparsable_code_yields_nothing() {
        for src in [
            "if x then\n",
            "local = 1\n",
            "end\n",
            "f(\n",
            "x\ny z\n",
            "local a = 1 2\n",
        ] {
            let source = Source::new("t.lua", src);
            assert!(resolve(&source).is_none(), "{src:?}");
        }
        let deep = format!("x = {}1{}\n", "(".repeat(500), ")".repeat(500));
        assert!(resolve(&Source::new("t.lua", deep)).is_none());
    }

    #[test]
    fn extended_syntax_parses() {
        assert_eq!(
            usage(
                "const a, b = 1, 2\nconst function f(p) return p end\nlocal c = 0\n\
                 c += a & b | ~c\nlocal g = |x, y| -> x ?? y ? f?.(x) : c?.d\n\
                 local h = z -> do return z end\nt[c] ~= g(1)\n"
            ),
            vec![
                entry("a", 1, 0),
                entry("b", 1, 0),
                entry("f", 1, 0),
                entry("p", 1, 0),
                entry("c", 4, 1),
                entry("x", 2, 0),
                entry("y", 1, 0),
                entry("g", 1, 0),
                entry("z", 1, 0),
                entry("h", 0, 0)
            ]
        );
    }

    #[test]
    fn short_function_parameters_belong_to_their_body() {
        let src = "local x = 1\nlocal f = x -> x\nprint(x)\n";
        let source = Source::new("t.lua", src);
        let resolution = resolve(&source).expect("parses");
        let parameter = &resolution.variables[1];
        assert_eq!(parameter.kind, DeclarationKind::Parameter);
        assert_eq!(parameter.shadows, Some(0));
        assert_eq!(&src[parameter.scope.clone()], "x -> x\n");
        assert_eq!(
            parameter.function.map(|token| &src[token.range()]),
            Some("x")
        );
        assert_eq!(resolution.variables[0].reads, 1);
    }

    #[test]
    fn glua_syntax_parses() {
        assert_eq!(
            usage(
                "local a = 1\nfor i = 1, 2 do if !a && a != 2 || a then continue end end\n\
                 // c\n/* d */\n"
            ),
            vec![entry("a", 3, 0), entry("i", 0, 0)]
        );
    }
}
