//! `Lint/ExtendedSyntax`.

use std::collections::HashSet;
use std::ops::Range;

use crate::offense::{Edit, Severity};
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, Function, FunctionForm, Statement, StatementKind, Visitor,
    binary_precedence, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::style::rewriting::{
    Placements, has_top_level_method_call, is_simple_target, never_false, placements,
};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags the syntax extensions of LuaJIT 3.0, which the LuaJIT of stock Garry's Mod rejects:
/// the bit operators (`&`, `|`, `~`, `<<`, `>>`, `~>>`), the ternary `c ? a : b`, safe
/// navigation (`a?.b`), nil coalescing (`a ?? b`), compound assignment (`x += 1`), `const`,
/// short functions (`|a| -> a`), `_` in numbers (`1_000`) and empty statements (a `;` that
/// does not end a statement). The GLua extensions (`!`, `!=`, `&&`, `||`, `continue`, C
/// comments) and `\u{XXXX}` escapes are not flagged.
///
/// The fix writes the plain Lua that does the same, where there is one: `const` becomes
/// `local`; the separators of a number are dropped; `x op= v` becomes `x = x op v` (with `v`
/// in parentheses where it would bind differently) when `x` is a name, a field of a name or an
/// index of a name by a name or a literal; a bit operation becomes the call of the `bit`
/// library (`a & b & c` to `bit.band(a, b, c)`, `~a` to `bit.bnot(a)`) where `bit` is the
/// global; `c ? a : b` becomes `c and a or b` when `a` is a literal that cannot be `false` or
/// `nil`; a short function becomes a `function` returning its expression (or with its block);
/// and an empty statement is removed. No fix is made where the plain Lua would end with a `)`
/// before a `(`, which would call it. The library converts numeric strings to numbers where the
/// operators raise an error. `?.` and `??` have no fix. Disabled by default; enable it for code
/// that must run on stock Garry's Mod. Files that do not parse are skipped.
pub struct ExtendedSyntax;

impl Reader for ExtendedSyntax {
    fn name(&self) -> &'static str {
        "Lint/ExtendedSyntax"
    }

    fn description(&self) -> &'static str {
        "Checks for the syntax extensions of LuaJIT 3.0, which stock Garry's Mod does not accept."
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let resolution = resolve(source);
        let mut finder = Finder {
            source,
            resolution: resolution.as_ref(),
            placements: placements(&block),
            chained: HashSet::new(),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        finder.tokens();
        finder
            .found
            .sort_by_key(|found| (found.covers.start, std::cmp::Reverse(found.covers.end)));
        for found in finder.found {
            let message = format!("{} the syntax extensions of LuaJIT 3.0.", found.subject);
            match found.fix {
                Some(edits) if !source.has_comment_in(&found.covers) => {
                    ctx.add_offense_with_fix(found.range, message, edits);
                }
                _ => {
                    ctx.add_offense(found.range, message);
                }
            }
        }
    }
}

/// One use of a syntax extension.
struct Found {
    /// Where the offense is reported.
    range: Range<usize>,
    /// The start of the message: what is used and `needs` or `need`.
    subject: String,
    /// The code the fix rewrites, which must hold no comment.
    covers: Range<usize>,
    /// The fix, when there is one.
    fix: Option<Vec<Edit>>,
}

/// Collects the uses of syntax extensions.
struct Finder<'a> {
    source: &'a Source,
    resolution: Option<&'a Resolution<'a>>,
    placements: Placements,
    /// The bit operations that are the left operand of the same operation, which the
    /// outermost one rewrites together with them.
    chained: HashSet<*const Expression>,
    found: Vec<Found>,
}

impl Finder<'_> {
    /// The text of `expression`.
    fn text(&self, expression: &Expression) -> &str {
        &self.source.text[expression.range.clone()]
    }

    /// Records the use at `range` described by `subject`, fixed by replacing `covers` with
    /// `replacement` when there is one.
    fn report(
        &mut self,
        range: Range<usize>,
        subject: String,
        covers: Range<usize>,
        replacement: Option<String>,
    ) {
        let called = text_ends_with_paren(&replacement)
            && self
                .source
                .code_tokens()
                .find(|token| token.start >= covers.end && !token.kind.is_comment())
                .is_some_and(|next| next.kind == TokenKind::LParen);
        let replacement = replacement.filter(|_| !called);
        let fix = replacement.map(|text| vec![Edit::replace(covers.clone(), text)]);
        self.found.push(Found {
            range,
            subject,
            covers,
            fix,
        });
    }

    /// Whether the global `bit` library is reachable at byte `offset` (no local named `bit` is
    /// visible there).
    fn bit_library(&self, offset: usize) -> bool {
        self.resolution
            .is_some_and(|resolution| resolution.visible_at("bit", offset).is_none())
    }

    /// Records the extensions only the tokens show: `const`, `?.`, `??` and number separators.
    fn tokens(&mut self) {
        let source = self.source;
        for token in source.code_tokens() {
            let text = source.text_of(token);
            match token.kind {
                TokenKind::Const => self.report(
                    token.range(),
                    "`const` needs".to_owned(),
                    token.range(),
                    Some("local".to_owned()),
                ),
                TokenKind::SafeNav | TokenKind::Coalesce => {
                    self.report(
                        token.range(),
                        format!("`{text}` needs"),
                        token.range(),
                        None,
                    );
                }
                TokenKind::Number if text.contains('_') => self.report(
                    token.range(),
                    format!("`{text}` needs"),
                    token.range(),
                    Some(text.replace('_', "")),
                ),
                _ => {}
            }
        }
    }

    /// Records a compound assignment, fixed as a plain assignment.
    fn compound(
        &mut self,
        statement: &Statement,
        target: &Expression,
        operator: &Token,
        value: &Expression,
    ) {
        let compound = self.source.text_of(operator);
        let binary = compound.strip_suffix('=').unwrap_or(compound);
        let replacement = is_simple_target(target).then(|| {
            let right = binary_precedence(operator_kind(binary)).map_or(0, |(_, right)| right);
            let grouped = match &value.kind {
                ExpressionKind::Binary { operator, .. } => {
                    binary_precedence(operator.kind).is_none_or(|(left, _)| left <= right)
                }
                ExpressionKind::Ternary { .. } => true,
                ExpressionKind::Function(function) => function.form.is_short(),
                _ => false,
            };
            let value = if grouped {
                format!("({})", self.text(value))
            } else {
                self.text(value).to_owned()
            };
            let target = self.text(target);
            format!("{target} = {target} {binary} {value}")
        });
        self.report(
            operator.range(),
            format!("`{compound}` needs"),
            statement.range.clone(),
            replacement,
        );
    }

    /// Records a bit operation, fixed as a call of the `bit` library; `a & b & c` (also with
    /// `|` and `~`) is one call.
    fn bit_operation(&mut self, expression: &Expression, operator: &Token) {
        let Some(function) = library_function(operator.kind) else {
            return;
        };
        let associative = matches!(
            operator.kind,
            TokenKind::Ampersand | TokenKind::Pipe | TokenKind::Tilde
        );
        let mut operands = Vec::new();
        let mut current = expression;
        while let ExpressionKind::Binary { left, right, .. } = &current.kind {
            operands.push(right.as_ref());
            let chained = matches!(
                &left.kind,
                ExpressionKind::Binary { operator: next, .. } if next.kind == operator.kind
            );
            if !(associative && chained) {
                operands.push(left.as_ref());
                break;
            }
            self.chained.insert(std::ptr::from_ref(left.as_ref()));
            current = left;
        }
        operands.reverse();
        let replacement = self
            .bit_library(expression.range.start)
            .then(|| self.call(function, &operands));
        self.report(
            operator.range(),
            format!("`{}` needs", self.source.text_of(operator)),
            expression.range.clone(),
            replacement,
        );
    }

    /// The call of `bit.function` with `operands` as its arguments: parentheses around an
    /// operand are dropped unless they cut a call down to one value, and a last operand that
    /// is a call keeps to one value in parentheses.
    fn call(&self, function: &str, operands: &[&Expression]) -> String {
        let count = operands.len();
        let arguments: Vec<String> = operands
            .iter()
            .enumerate()
            .map(|(index, operand)| {
                if let ExpressionKind::Parenthesized(inner) = &operand.kind
                    && !inner.is_multi_valued()
                {
                    return self.text(inner).to_owned();
                }
                if index + 1 == count && operand.is_multi_valued() && !self.is_bit_call(operand) {
                    return format!("({})", self.text(operand));
                }
                self.text(operand).to_owned()
            })
            .collect();
        format!("bit.{function}({})", arguments.join(", "))
    }

    /// Whether `expression` calls a function of the `bit` library, which returns one value.
    fn is_bit_call(&self, expression: &Expression) -> bool {
        expression
            .as_call(self.source)
            .is_some_and(|(name, _)| name.starts_with("bit."))
    }

    /// Records a ternary, fixed as `c and a or b` when `a` cannot be `false` or `nil`.
    fn ternary(&mut self, expression: &Expression, question: &Token, parts: [&Expression; 3]) {
        let [condition, consequent, alternative] = parts;
        let replacement = never_false(consequent).then(|| {
            let condition = match &condition.kind {
                ExpressionKind::Binary { operator, .. }
                    if binary_precedence(operator.kind).is_some_and(|(left, _)| left < 2) =>
                {
                    format!("({})", self.text(condition))
                }
                _ => self.text(condition).to_owned(),
            };
            let no_method_calls = self
                .placements
                .of(expression)
                .is_some_and(|placement| placement.no_method_calls);
            let grouped = matches!(alternative.kind, ExpressionKind::Ternary { .. })
                || alternative
                    .as_function()
                    .is_some_and(|function| function.form.is_short())
                || (no_method_calls && has_top_level_method_call(self.source, alternative));
            let alternative = if grouped {
                format!("({})", self.text(alternative))
            } else {
                self.text(alternative).to_owned()
            };
            format!("{condition} and {} or {alternative}", self.text(consequent))
        });
        self.report(
            question.range(),
            "The ternary `? :` needs".to_owned(),
            expression.range.clone(),
            replacement,
        );
    }

    /// Records a short function, fixed as a `function`.
    fn short_function(&mut self, expression: &Expression, function: &Function) {
        let source = self.source;
        let mut parameters: Vec<&str> = function
            .parameters
            .iter()
            .map(|parameter| source.text_of(parameter))
            .collect();
        if let Some(vararg) = &function.vararg {
            parameters.push(source.text_of(vararg));
        }
        let head = format!("function({})", parameters.join(", "));
        let (covers, replacement) = match function.form {
            FunctionForm::Expression { .. } => {
                let body = match function
                    .body
                    .statements
                    .first()
                    .map(|statement| &statement.kind)
                {
                    Some(StatementKind::Return { values, .. }) => values
                        .first()
                        .map_or("", |value| &source.text[value.range.clone()]),
                    _ => "",
                };
                (
                    expression.range.clone(),
                    format!("{head} return {body} end"),
                )
            }
            FunctionForm::Block { .. } => {
                let body_start = function.body.range.start;
                let open = source
                    .code_tokens()
                    .filter(|token| {
                        token.end <= body_start && token.start >= expression.range.start
                    })
                    .last()
                    .filter(|token| token.kind == TokenKind::Do);
                match open {
                    Some(open) => (expression.range.start..open.end, head),
                    None => return,
                }
            }
            FunctionForm::Long => return,
        };
        self.report(
            function.keyword.range(),
            "Short functions need".to_owned(),
            covers,
            Some(replacement),
        );
    }

    /// Records the `;`s of `block` that end no statement.
    fn empty_statements(&mut self, block: &Block) {
        let mut ends_statement = false;
        for statement in &block.statements {
            if statement.kind != StatementKind::Empty {
                ends_statement = true;
                continue;
            }
            if !ends_statement {
                self.report(
                    statement.range.clone(),
                    "Empty statements need".to_owned(),
                    statement.range.clone(),
                    Some(String::new()),
                );
            }
            ends_statement = false;
        }
    }
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn enter_block(&mut self, block: &'t Block) {
        self.empty_statements(block);
    }

    fn statement(&mut self, statement: &'t Statement) {
        if let StatementKind::CompoundAssignment {
            target,
            operator,
            value,
        } = &statement.kind
        {
            self.compound(statement, target, operator, value);
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Binary { operator, .. }
                if !self.chained.contains(&std::ptr::from_ref(expression)) =>
            {
                self.bit_operation(expression, operator);
            }
            ExpressionKind::Unary { operator, operand } if operator.kind == TokenKind::Tilde => {
                let replacement = self
                    .bit_library(expression.range.start)
                    .then(|| self.call("bnot", &[operand.as_ref()]));
                self.report(
                    operator.range(),
                    "`~` needs".to_owned(),
                    expression.range.clone(),
                    replacement,
                );
            }
            ExpressionKind::Ternary {
                condition,
                question,
                consequent,
                alternative,
                ..
            } => self.ternary(expression, question, [condition, consequent, alternative]),
            ExpressionKind::Function(function) if function.form.is_short() => {
                self.short_function(expression, function);
            }
            _ => {}
        }
    }
}

/// Whether the replacement `text` ends with a `)`, which a `(` after it would call.
fn text_ends_with_paren(text: &Option<String>) -> bool {
    text.as_ref().is_some_and(|text| text.ends_with(')'))
}

/// The token kind of the binary operator spelled `text`.
fn operator_kind(text: &str) -> TokenKind {
    match text {
        "+" => TokenKind::Plus,
        "-" => TokenKind::Minus,
        "*" => TokenKind::Star,
        "/" => TokenKind::Slash,
        "%" => TokenKind::Percent,
        ".." => TokenKind::Concat,
        "&" => TokenKind::Ampersand,
        "|" => TokenKind::Pipe,
        "~" => TokenKind::Tilde,
        "<<" => TokenKind::Shl,
        ">>" => TokenKind::Shr,
        _ => TokenKind::Sar,
    }
}

/// The function of the `bit` library computing the binary bit operator `kind`.
fn library_function(kind: TokenKind) -> Option<&'static str> {
    Some(match kind {
        TokenKind::Ampersand => "band",
        TokenKind::Pipe => "bor",
        TokenKind::Tilde => "bxor",
        TokenKind::Shl => "lshift",
        TokenKind::Shr => "rshift",
        TokenKind::Sar => "arshift",
        _ => return None,
    })
}

inventory::submit! { Registration(|| Box::new(ExtendedSyntax)) }

#[cfg(test)]
mod tests {
    use crate::offense::Severity;
    use crate::testing::*;

    const READER: &str = "Lint/ExtendedSyntax";
    const ENABLED: &str = "Enabled: true";

    /// `(line, col, message)` of the offenses for `src` with the reader enabled.
    fn found(src: &str) -> Vec<(u32, u32, String)> {
        inspect_with(READER, src, ENABLED)
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect()
    }

    /// `src` corrected with the reader enabled.
    fn corrected(src: &str) -> String {
        autocorrect_with(READER, src, ENABLED)
    }

    /// The message for `subject`.
    fn needs(subject: &str) -> String {
        format!("{subject} the syntax extensions of LuaJIT 3.0.")
    }

    #[test]
    fn is_disabled_by_default_and_an_error() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
        let offenses = inspect_with(READER, "x = a ?? b\n", ENABLED);
        assert_eq!(offenses[0].severity, Severity::Error);
    }

    #[test]
    fn flags_every_extension() {
        assert_eq!(
            found(
                "const a = 1_000\nlocal b = a & 1 | ~a\nlocal c = a ? 1 : 2\nlocal d = a?.b ?? 3\n\
                 b += 1;;\nf(|x| -> x)\n"
            ),
            vec![
                (1, 1, needs("`const` needs")),
                (1, 11, needs("`1_000` needs")),
                (2, 13, needs("`&` needs")),
                (2, 17, needs("`|` needs")),
                (2, 19, needs("`~` needs")),
                (3, 13, needs("The ternary `? :` needs")),
                (4, 12, needs("`?.` needs")),
                (4, 16, needs("`??` needs")),
                (5, 3, needs("`+=` needs")),
                (5, 8, needs("Empty statements need")),
                (6, 3, needs("Short functions need")),
            ]
        );
    }

    #[test]
    fn accepts_glua_and_plain_lua() {
        assert!(
            found(
                "if !a && b != c || d then continue end\n// c\n/* d */\nlocal s = '\\u{48}'\n\
             x = a and 1 or 2; y = bit.band(a, 1)\nreturn;\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn lowers_declarations_numbers_and_statements() {
        assert_eq!(
            corrected("const a, b = 1_000, 0x_FF\nconst function f() end\n;; g() ;; h();\n"),
            "local a, b = 1000, 0xFF\nlocal function f() end\n; g() ; h();\n"
        );
    }

    #[test]
    fn lowers_compound_assignments() {
        assert_eq!(
            corrected("x += 1\nt.x -= a - b\nt[1] *= a * b\ns ..= a .. b\nt[k] /= -a\n"),
            "x = x + 1\nt.x = t.x - (a - b)\nt[1] = t[1] * (a * b)\ns = s .. a .. b\nt[k] = t[k] / -a\n"
        );
        assert_eq!(
            corrected("x ~= 1\ny <<= c ? 1 : 2\n"),
            "x = bit.bxor(x, 1)\ny = bit.lshift(y, c and 1 or 2)\n"
        );
        let offenses = inspect_with(READER, "t.a.b += 1\nf().x ..= 's'\n", ENABLED);
        assert_eq!(offenses.len(), 2);
        assert!(offenses.iter().all(|offense| !offense.correctable()));
    }

    #[test]
    fn lowers_bit_operators() {
        assert_eq!(
            corrected(
                "x = a & b & c\ny = (a | b) ~ ~c\nz = a << 1 >> 2 ~>> 3\nw = 1 + (f() & g())\n"
            ),
            "x = bit.band(a, b, c)\ny = bit.bxor(bit.bor(a, b), bit.bnot(c))\n\
             z = bit.arshift(bit.rshift(bit.lshift(a, 1), 2), 3)\nw = 1 + (bit.band(f(), (g())))\n"
        );
        let offenses = inspect_with(READER, "local bit = {}\nx = a & b\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }

    #[test]
    fn lowers_ternaries_with_literal_consequents() {
        assert_eq!(
            corrected("x = c ? 'a' : b\ny = a or b ? 1 : c ? 2 : 3\nz = c ? {} : nil\n"),
            "x = c and 'a' or b\ny = (a or b) and 1 or (c and 2 or 3)\nz = c and {} or nil\n"
        );
        let offenses = inspect_with(READER, "x = c ? a : b\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }

    #[test]
    fn lowers_short_functions() {
        assert_eq!(
            corrected(
                "f(|a, b| -> a + b, x -> x, || -> 1, |...| -> select('#', ...))\n\
                 g(|a| -> do\n  return a\nend)\n"
            ),
            "f(function(a, b) return a + b end, function(x) return x end, function() return 1 end, \
             function(...) return select('#', ...) end)\ng(function(a)\n  return a\nend)\n"
        );
    }

    #[test]
    fn leaves_lowerings_that_a_parenthesis_would_call() {
        for src in [
            "x -= a - 1\n(f)()\n",
            "x = a & 1\n(f)()\n",
            "local x = c ? 1 : d ? 2 : 3\n(f)()\n",
        ] {
            let offenses = inspect_with(READER, src, ENABLED);
            assert!(
                offenses.iter().any(|offense| !offense.correctable()),
                "{src}"
            );
        }
    }

    #[test]
    fn lowers_everything_to_plain_lua() {
        let src = "const MAX = 10_000\nlocal t = {}\nt.n = 0\nt.n += 1\nt.flags = t.n & 0xF | 1 << 4\n\
                   table.sort(t, |a, b| -> a > b)\nlocal label = t.n > 0 ? 'some' : 'none'\n";
        let lowered = corrected(src);
        assert!(found(&lowered).is_empty(), "{lowered}");
        assert_eq!(
            lowered,
            "local MAX = 10000\nlocal t = {}\nt.n = 0\nt.n = t.n + 1\n\
             t.flags = bit.bor(bit.band(t.n, 0xF), bit.lshift(1, 4))\n\
             table.sort(t, function(a, b) return a > b end)\nlocal label = t.n > 0 and 'some' or 'none'\n"
        );
    }
}
