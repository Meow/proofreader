//! Helpers shared by the readers that rewrite code into or out of the syntax extensions of
//! LuaJIT 3.0 (`Style/CompoundAssignment`, `Style/BitOperators`, `Style/TernaryOperator`,
//! `Style/ShortFunction`, `Lint/ExtendedSyntax`): where every expression of a file stands, and
//! the structure of an expression for checking that a rewrite parses as intended.

use std::collections::HashMap;

use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, FieldKey, Function, FunctionForm, StatementKind, parse,
};
use crate::source::Source;
use crate::token::TokenKind;

/// What an expression is to the construct around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A value of a `local` or `const` statement.
    LocalValue,
    /// A value of an assignment.
    AssignedValue,
    /// The value of a compound assignment.
    CompoundValue,
    /// A target of an assignment or compound assignment.
    Target,
    /// A value of a `return`, or the expression of a short function.
    ReturnValue,
    /// The value of a table field.
    FieldValue,
    /// The bracketed key of a table field or an index.
    Key,
    /// A call argument.
    Argument,
    /// The expression inside parentheses.
    Parenthesized,
    /// The condition of an `if`, `elseif`, `while`, `until` or ternary.
    Condition,
    /// The value of a ternary when its condition holds.
    Consequent,
    /// The value of a ternary when its condition does not hold.
    Alternative,
    /// An operand of a binary or unary operator.
    Operand {
        /// The operator.
        operator: TokenKind,
        /// Which operand it is.
        side: Side,
    },
    /// The object of a field, index or method call, or the callee of a call.
    Prefix,
    /// A value in the header of a `for` loop.
    LoopValue,
    /// A call standing as a statement.
    Statement,
    /// Lua spliced into Lumen markup.
    Splice,
}

/// Which operand of an operator an expression is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The left operand of a binary operator.
    Left,
    /// The right operand of a binary operator.
    Right,
    /// The operand of a unary operator.
    Unary,
}

/// Where an expression stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// What the expression is to the construct around it.
    pub role: Role,
    /// Whether LuaJIT reads no method calls at the level of the expression: inside the first
    /// branch of a ternary and what it holds without brackets (operands, the expression of a
    /// short function), where `a:b` is read as the end of the branch.
    pub no_method_calls: bool,
}

/// The placement of every expression of a parsed file.
#[derive(Debug, Default)]
pub struct Placements(HashMap<*const Expression, Placement>);

impl Placements {
    /// The placement of `expression`, which must belong to the block the placements were made
    /// from.
    pub fn of(&self, expression: &Expression) -> Option<Placement> {
        self.0.get(&std::ptr::from_ref(expression)).copied()
    }

    /// Records the placement of `expression` and of everything in it.
    fn expression(&mut self, expression: &Expression, role: Role, no_method_calls: bool) {
        self.0.insert(
            std::ptr::from_ref(expression),
            Placement {
                role,
                no_method_calls,
            },
        );
        match &expression.kind {
            ExpressionKind::Nil
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Vararg
            | ExpressionKind::Opaque
            | ExpressionKind::Name(_) => {}
            ExpressionKind::Function(function) => self.function(function, no_method_calls),
            ExpressionKind::Table(table) => {
                for field in &table.fields {
                    if let FieldKey::Bracketed(key) = &field.key {
                        self.expression(key, Role::Key, false);
                    }
                    self.expression(&field.value, Role::FieldValue, false);
                }
            }
            ExpressionKind::Markup(splices) => {
                for splice in splices {
                    self.expression(splice, Role::Splice, false);
                }
            }
            ExpressionKind::Parenthesized(inner) => {
                self.expression(inner, Role::Parenthesized, false);
            }
            ExpressionKind::Field { object, .. } | ExpressionKind::SafeField { object, .. } => {
                self.expression(object, Role::Prefix, no_method_calls);
            }
            ExpressionKind::Index { object, key } | ExpressionKind::SafeIndex { object, key } => {
                self.expression(object, Role::Prefix, no_method_calls);
                self.expression(key, Role::Key, false);
            }
            ExpressionKind::Call { callee, arguments }
            | ExpressionKind::SafeCall { callee, arguments } => {
                self.expression(callee, Role::Prefix, no_method_calls);
                for argument in &arguments.values {
                    self.expression(argument, Role::Argument, false);
                }
            }
            ExpressionKind::MethodCall {
                object, arguments, ..
            }
            | ExpressionKind::SafeMethodCall {
                object, arguments, ..
            } => {
                self.expression(object, Role::Prefix, no_method_calls);
                for argument in &arguments.values {
                    self.expression(argument, Role::Argument, false);
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                let role = Role::Operand {
                    operator: operator.kind,
                    side: Side::Unary,
                };
                self.expression(operand, role, no_method_calls);
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let role = |side| Role::Operand {
                    operator: operator.kind,
                    side,
                };
                self.expression(left, role(Side::Left), no_method_calls);
                self.expression(right, role(Side::Right), no_method_calls);
            }
            ExpressionKind::Ternary {
                condition,
                consequent,
                alternative,
                ..
            } => {
                self.expression(condition, Role::Condition, no_method_calls);
                self.expression(consequent, Role::Consequent, true);
                self.expression(alternative, Role::Alternative, false);
            }
        }
    }

    /// Records the placements inside `function`, whose expression body (for a short function
    /// written with one) inherits `no_method_calls`.
    fn function(&mut self, function: &Function, no_method_calls: bool) {
        if let FunctionForm::Expression { .. } = function.form
            && let Some(statement) = function.body.statements.first()
            && let StatementKind::Return { values, .. } = &statement.kind
        {
            for value in values {
                self.expression(value, Role::ReturnValue, no_method_calls);
            }
            return;
        }
        self.block(&function.body);
    }

    /// Records the placements inside `block`.
    fn block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.statement(&statement.kind);
        }
    }

    /// Records the placements inside one statement.
    fn statement(&mut self, statement: &StatementKind) {
        let each = |placements: &mut Self, values: &[Expression], role: Role| {
            for value in values {
                placements.expression(value, role, false);
            }
        };
        match statement {
            StatementKind::Empty
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Goto { .. }
            | StatementKind::Label { .. } => {}
            StatementKind::Local { values, .. } => each(self, values, Role::LocalValue),
            StatementKind::Return { values, .. } => each(self, values, Role::ReturnValue),
            StatementKind::LocalFunction { function, .. }
            | StatementKind::Function { function, .. } => self.function(function, false),
            StatementKind::Assignment {
                targets, values, ..
            } => {
                each(self, targets, Role::Target);
                each(self, values, Role::AssignedValue);
            }
            StatementKind::CompoundAssignment { target, value, .. } => {
                self.expression(target, Role::Target, false);
                self.expression(value, Role::CompoundValue, false);
            }
            StatementKind::Call(call) => self.expression(call, Role::Statement, false),
            StatementKind::Do { body } => self.block(body),
            StatementKind::While { condition, body } => {
                self.expression(condition, Role::Condition, false);
                self.block(body);
            }
            StatementKind::Repeat { body, condition } => {
                self.block(body);
                self.expression(condition, Role::Condition, false);
            }
            StatementKind::If {
                clauses, otherwise, ..
            } => {
                for clause in clauses {
                    self.expression(&clause.condition, Role::Condition, false);
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
                self.expression(start, Role::LoopValue, false);
                self.expression(limit, Role::LoopValue, false);
                if let Some(step) = step {
                    self.expression(step, Role::LoopValue, false);
                }
                self.block(body);
            }
            StatementKind::GenericFor { values, body, .. } => {
                each(self, values, Role::LoopValue);
                self.block(body);
            }
        }
    }
}

/// The placement of every expression of `block`.
pub fn placements(block: &Block) -> Placements {
    let mut placements = Placements::default();
    placements.block(block);
    placements
}

/// The structure of `expression` as text: every operation and suffix in parentheses with its
/// operator, every other expression as written (without whitespace). Two expressions with the
/// same shape parse the same way.
pub fn shape(source: &Source, expression: &Expression) -> String {
    let text = |range: std::ops::Range<usize>| -> String {
        source
            .text
            .get(range)
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    };
    match &expression.kind {
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => format!(
            "({} {} {})",
            shape(source, left),
            source.text_of(operator),
            shape(source, right)
        ),
        ExpressionKind::Unary { operator, operand } => {
            format!("({} {})", source.text_of(operator), shape(source, operand))
        }
        ExpressionKind::Ternary {
            condition,
            consequent,
            alternative,
            ..
        } => format!(
            "({} ? {} : {})",
            shape(source, condition),
            shape(source, consequent),
            shape(source, alternative)
        ),
        ExpressionKind::Parenthesized(inner) => format!("[{}]", shape(source, inner)),
        ExpressionKind::Field { object, name } => {
            format!("({} . {})", shape(source, object), source.text_of(name))
        }
        ExpressionKind::SafeField { object, name } => {
            format!("({} ?. {})", shape(source, object), source.text_of(name))
        }
        ExpressionKind::Index { object, key } => {
            format!("({} [{}])", shape(source, object), shape(source, key))
        }
        ExpressionKind::SafeIndex { object, key } => {
            format!("({} ?.[{}])", shape(source, object), shape(source, key))
        }
        ExpressionKind::Call { callee, arguments } => {
            format!(
                "({} {})",
                shape(source, callee),
                text(arguments.range.clone())
            )
        }
        ExpressionKind::SafeCall { callee, arguments } => {
            format!(
                "({} ?.{})",
                shape(source, callee),
                text(arguments.range.clone())
            )
        }
        ExpressionKind::MethodCall {
            object,
            name,
            arguments,
        } => format!(
            "({} : {} {})",
            shape(source, object),
            source.text_of(name),
            text(arguments.range.clone())
        ),
        ExpressionKind::SafeMethodCall {
            object,
            name,
            arguments,
            navigation,
        } => {
            let colon = if navigation.start < name.start {
                "?.:"
            } else {
                ":?."
            };
            format!(
                "({} {colon} {} {})",
                shape(source, object),
                source.text_of(name),
                text(arguments.range.clone())
            )
        }
        _ => text(expression.range.clone()),
    }
}

/// The shape of the value of the single compound assignment, assignment or `return` that `code`
/// consists of, when `code` parses as one; for checking how a rewritten statement reads.
pub fn statement_value_shape(code: &str) -> Option<String> {
    let source = Source::new("rewrite.lua", code);
    let block = parse(&source).ok()?;
    let [statement] = block.statements.as_slice() else {
        return None;
    };
    let value = match &statement.kind {
        StatementKind::CompoundAssignment { value, .. } => value,
        StatementKind::Assignment { values, .. } | StatementKind::Return { values, .. } => {
            match values.as_slice() {
                [value] => value,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(shape(&source, value))
}

/// Whether `expression` reads the same value every time and costs no call: a name, a field of
/// a name or an index of a name by a name or a literal (`x`, `t.x`, `t[k]`, `t[1]`). Writing it
/// twice or once makes no difference.
pub fn is_simple_target(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Name(_) => true,
        ExpressionKind::Field { object, .. } => matches!(object.kind, ExpressionKind::Name(_)),
        ExpressionKind::Index { object, key } => {
            matches!(object.kind, ExpressionKind::Name(_))
                && matches!(
                    key.kind,
                    ExpressionKind::Name(_)
                        | ExpressionKind::Number
                        | ExpressionKind::String(_)
                        | ExpressionKind::True
                        | ExpressionKind::False
                )
        }
        _ => false,
    }
}

/// Whether `expression` is a literal that is neither `false` nor `nil`: a number (also
/// negated), a string, a table constructor or `true`.
pub fn never_false(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Number
        | ExpressionKind::String(_)
        | ExpressionKind::Table(_)
        | ExpressionKind::True => true,
        ExpressionKind::Unary { operator, operand } => {
            operator.kind == TokenKind::Minus && operand.kind == ExpressionKind::Number
        }
        _ => false,
    }
}

/// Whether the code of `expression` holds a `:` of a method call outside any brackets, block
/// or function body of its own, which LuaJIT would read differently in the first branch of a
/// ternary.
pub fn has_top_level_method_call(source: &Source, expression: &Expression) -> bool {
    let mut depth = 0usize;
    for token in source.code_tokens() {
        if token.start < expression.range.start {
            continue;
        }
        if token.start >= expression.range.end {
            break;
        }
        match token.kind {
            TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::Function
            | TokenKind::Do
            | TokenKind::If
            | TokenKind::Repeat => depth += 1,
            TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
            | TokenKind::End
            | TokenKind::Until => depth = depth.saturating_sub(1),
            TokenKind::Colon if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::lint::ast::{Visitor, walk};

    /// `(text, role, no_method_calls)` of every expression of `src` whose text is a single name.
    fn names(src: &str) -> Vec<(String, Role, bool)> {
        struct Names<'t>(Vec<&'t Expression>);
        impl<'t> Visitor<'t> for Names<'t> {
            fn expression(&mut self, expression: &'t Expression) {
                if matches!(expression.kind, ExpressionKind::Name(_)) {
                    self.0.push(expression);
                }
            }
        }
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let found = placements(&block);
        let mut names = Names(Vec::new());
        walk(&block, &mut names);
        names
            .0
            .into_iter()
            .map(|expression| {
                let placement = found.of(expression).expect("placed");
                (
                    src[expression.range.clone()].to_owned(),
                    placement.role,
                    placement.no_method_calls,
                )
            })
            .collect()
    }

    #[test]
    fn places_expressions() {
        use Role::*;
        let operand = |side| Operand {
            operator: TokenKind::Plus,
            side,
        };
        assert_eq!(
            names("local a = b + c\nd, e.f = g(h), {i, [j] = k}\nreturn (l) ? m : n\n"),
            vec![
                ("b".to_owned(), operand(Side::Left), false),
                ("c".to_owned(), operand(Side::Right), false),
                ("d".to_owned(), Target, false),
                ("e".to_owned(), Prefix, false),
                ("g".to_owned(), Prefix, false),
                ("h".to_owned(), Argument, false),
                ("i".to_owned(), FieldValue, false),
                ("j".to_owned(), Key, false),
                ("k".to_owned(), FieldValue, false),
                ("l".to_owned(), Parenthesized, false),
                ("m".to_owned(), Consequent, true),
                ("n".to_owned(), Alternative, false),
            ]
        );
    }

    #[test]
    fn method_calls_are_not_read_in_first_branches() {
        use Role::*;
        assert_eq!(
            names("x = c ? || -> a + f(b) : d\n"),
            vec![
                ("x".to_owned(), Target, false),
                ("c".to_owned(), Condition, false),
                (
                    "a".to_owned(),
                    Operand {
                        operator: TokenKind::Plus,
                        side: Side::Left
                    },
                    true
                ),
                ("f".to_owned(), Prefix, true),
                ("b".to_owned(), Argument, false),
                ("d".to_owned(), Alternative, false),
            ]
        );
    }

    #[test]
    fn shapes_tell_parses_apart() {
        assert_eq!(
            statement_value_shape("x += a * b"),
            statement_value_shape("x = a*b")
        );
        assert_ne!(
            statement_value_shape("x = (a + b) * c"),
            statement_value_shape("x = a + b * c")
        );
        assert_eq!(
            statement_value_shape("return a ? b : c"),
            Some("(a ? b : c)".to_owned())
        );
        assert!(statement_value_shape("x = 1 y = 2").is_none());
    }

    #[test]
    fn simple_targets() {
        let source = Source::new(
            "t.lua",
            "x, t.x, t[k], t[1], t.a.b, t[f()], f().x, (t).x = 1\n",
        );
        let block = parse(&source).expect("parses");
        let StatementKind::Assignment { targets, .. } = &block.statements[0].kind else {
            panic!("assignment");
        };
        let simple: Vec<bool> = targets.iter().map(is_simple_target).collect();
        assert_eq!(
            simple,
            vec![true, true, true, true, false, false, false, false]
        );
    }

    #[test]
    fn finds_top_level_method_calls() {
        let source = Source::new(
            "t.lua",
            "x = { o:m(), f(o:m()), function() return o:m() end, || -> do o:m() end }\n",
        );
        let block = parse(&source).expect("parses");
        let StatementKind::Assignment { values, .. } = &block.statements[0].kind else {
            panic!("assignment");
        };
        let ExpressionKind::Table(table) = &values[0].kind else {
            panic!("table");
        };
        let found: Vec<bool> = table
            .fields
            .iter()
            .map(|field| has_top_level_method_call(&source, &field.value))
            .collect();
        assert_eq!(found, vec![true, false, false, false]);
    }
}
