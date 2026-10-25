//! `Lint/RedefinedLabel`.

use yaml_rust2::Yaml;

use crate::offense::Severity;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, FieldKey, Function, Statement, StatementKind, parse,
};
use crate::source::Source;
use crate::token::Token;

/// Flags a `::label::` whose name another label of the same block already uses, which LuaJIT
/// rejects (`duplicate label`).
///
/// With `AllowNestedRedefinition: false`, a label that reuses the name of a label in an
/// enclosing block of the same function is flagged too: LuaJIT accepts it, but a `goto` then
/// jumps to a different label depending on where it stands. Function bodies start afresh. Files
/// that do not parse are skipped.
pub struct RedefinedLabel;

impl Reader for RedefinedLabel {
    fn name(&self) -> &'static str {
        "Lint/RedefinedLabel"
    }

    fn description(&self) -> &'static str {
        "Checks for labels defined twice in the same block."
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("AllowNestedRedefinition", Yaml::Boolean(true))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(chunk) = parse(source) else {
            return;
        };
        let mut checker = Checker {
            source,
            nested: !ctx.option_bool("AllowNestedRedefinition", true),
            scopes: Vec::new(),
            found: Vec::new(),
        };
        checker.block(&chunk);
        for (token, message) in checker.found {
            ctx.add_offense(token.range(), message);
        }
    }
}

/// Walks the tree keeping the labels of the open blocks of the current function.
struct Checker<'a> {
    source: &'a Source,
    nested: bool,
    /// The labels of every open block of the current function, innermost last.
    scopes: Vec<Vec<Token>>,
    found: Vec<(Token, String)>,
}

impl Checker<'_> {
    /// Checks the labels of `block` and everything nested in it.
    fn block(&mut self, block: &Block) {
        self.scopes.push(Vec::new());
        for statement in &block.statements {
            self.statement(statement);
        }
        self.scopes.pop();
    }

    /// Checks a function body with no labels of the enclosing function visible.
    fn function(&mut self, function: &Function) {
        let outer = std::mem::take(&mut self.scopes);
        self.block(&function.body);
        self.scopes = outer;
    }

    /// Checks one statement.
    fn statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Label { name } => self.label(*name),
            StatementKind::Local { values, .. } | StatementKind::Return { values, .. } => {
                self.expressions(values);
            }
            StatementKind::LocalFunction { function, .. }
            | StatementKind::Function { function, .. } => self.function(function),
            StatementKind::Assignment {
                targets, values, ..
            } => {
                self.expressions(targets);
                self.expressions(values);
            }
            StatementKind::Call(call) => self.expression(call),
            StatementKind::Do { body } => self.block(body),
            StatementKind::While { condition, body } => {
                self.expression(condition);
                self.block(body);
            }
            StatementKind::Repeat { body, condition } => {
                self.block(body);
                self.expression(condition);
            }
            StatementKind::If {
                clauses, otherwise, ..
            } => {
                for clause in clauses {
                    self.expression(&clause.condition);
                    self.block(&clause.body);
                }
                if let Some((_, body)) = otherwise {
                    self.block(body);
                }
            }
            StatementKind::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.expression(start);
                self.expression(limit);
                if let Some(step) = step {
                    self.expression(step);
                }
                self.block(body);
            }
            StatementKind::GenericFor { values, body, .. } => {
                self.expressions(values);
                self.block(body);
            }
            StatementKind::Empty
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Goto { .. } => {}
        }
    }

    /// Records the label `name`, reporting it when a visible label has the same name.
    fn label(&mut self, name: Token) {
        let text = self.source.text_of(&name);
        let searched = if self.nested {
            self.scopes.len()
        } else {
            self.scopes.len().min(1)
        };
        let earlier = self
            .scopes
            .iter()
            .rev()
            .take(searched)
            .flatten()
            .find(|label| self.source.text_of(label) == text);
        if let Some(earlier) = earlier {
            self.found.push((
                name,
                format!(
                    "Label `{text}` is already defined on line {}.",
                    earlier.line
                ),
            ));
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name);
        }
    }

    /// Checks the functions inside a list of expressions.
    fn expressions(&mut self, expressions: &[Expression]) {
        for expression in expressions {
            self.expression(expression);
        }
    }

    /// Checks the functions inside an expression.
    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Function(function) => self.function(function),
            ExpressionKind::Table(table) => {
                for field in &table.fields {
                    if let FieldKey::Bracketed(key) = &field.key {
                        self.expression(key);
                    }
                    self.expression(&field.value);
                }
            }
            ExpressionKind::Markup(splices) => self.expressions(splices),
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Field { object: inner, .. }
            | ExpressionKind::Unary { operand: inner, .. } => self.expression(inner),
            ExpressionKind::Index { object, key } => {
                self.expression(object);
                self.expression(key);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.expression(callee);
                self.expressions(&arguments.values);
            }
            ExpressionKind::MethodCall {
                object, arguments, ..
            } => {
                self.expression(object);
                self.expressions(&arguments.values);
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExpressionKind::Nil
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Vararg
            | ExpressionKind::Opaque
            | ExpressionKind::Name(_) => {}
        }
    }
}

inventory::submit! { Registration(|| Box::new(RedefinedLabel)) }

#[cfg(test)]
mod tests {
    use crate::offense::Severity;
    use crate::testing::*;

    const READER: &str = "Lint/RedefinedLabel";

    #[test]
    fn flags_labels_defined_twice_in_a_block() {
        expect_offense(
            READER,
            "::top::\nf()\n::top::\n",
            3,
            3,
            "Label `top` is already defined on line 1.",
        );
        expect_offense(
            READER,
            "for i = 1, 2 do\n  ::skip::\n  ::skip::\nend\n",
            3,
            5,
            "Label `skip` is already defined on line 2.",
        );
        assert_eq!(
            inspect(READER, "::a:: ::a::\n")[0].severity,
            Severity::Error
        );
    }

    #[test]
    fn accepts_distinct_labels_and_separate_blocks() {
        expect_no_offenses(READER, "::a::\n::b::\n");
        expect_no_offenses(
            READER,
            "for i = 1, 2 do ::continue_loop:: end\nfor i = 1, 2 do ::continue_loop:: end\n",
        );
        expect_no_offenses(
            READER,
            "if x then ::a:: elseif y then ::a:: else ::a:: end\n",
        );
    }

    #[test]
    fn nested_redefinition_is_optional() {
        let src = "::a::\ndo\n  ::a::\nend\n";
        expect_no_offenses(READER, src);
        let offenses = inspect_with(READER, src, "AllowNestedRedefinition: false");
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            (
                offenses[0].line,
                offenses[0].col,
                offenses[0].message.as_str()
            ),
            (3, 5, "Label `a` is already defined on line 1.")
        );
    }

    #[test]
    fn functions_start_afresh() {
        let src = "::a::\nlocal f = function()\n  ::a::\nend\nlocal function g() ::a:: end\n\
                   t = { h = function() ::a:: end }\n";
        expect_no_offenses(READER, src);
        assert!(inspect_with(READER, src, "AllowNestedRedefinition: false").is_empty());
        expect_offense(
            READER,
            "f(function()\n  ::a:: ::a::\nend)\n",
            2,
            11,
            "Label `a` is already defined on line 2.",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "::a:: ::a::\nf(\n");
    }
}
