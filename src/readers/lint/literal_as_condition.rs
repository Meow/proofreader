//! `Lint/LiteralAsCondition`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::source::Source;
use crate::token::TokenKind;

/// The functions that raise an error unless their first argument is truthy.
const ASSERT_FUNCTIONS: [&str; 1] = ["assert"];

/// Flags conditions whose outcome the literals in them already decide, so a branch always or
/// never runs.
///
/// A literal is always truthy (`true`, a number, `0` included, a string, a table constructor or
/// a function, also a negative number) or always falsy (`false`, `nil`). The outcome is followed through parentheses,
/// `not`/`!`, `and`, `or`, `??` and ternaries: `x or true` is always truthy and `x and false`
/// always falsy, whatever `x` is. Checked are the conditions of `if` and `elseif`, the
/// conditions of ternaries, the conditions of `while` loops that never run (`while true do`
/// stays allowed), and the first argument of the `AssertFunctions` when it always passes
/// (`assert(false, message)` raises on purpose and stays allowed). `repeat ... until` loops are
/// left alone: `until false` loops forever and `until true` runs the body once, both on
/// purpose. Files that do not parse are skipped.
pub struct LiteralAsCondition;

impl Reader for LiteralAsCondition {
    fn name(&self) -> &'static str {
        "Lint/LiteralAsCondition"
    }

    fn description(&self) -> &'static str {
        "Checks for conditions that are always truthy or always falsy because of literals."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("AssertFunctions", yaml_strings(&ASSERT_FUNCTIONS))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            asserts: ctx.option_str_list("AssertFunctions", &ASSERT_FUNCTIONS),
            found: Vec::new(),
        };
        walk(block, &mut finder);
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Whether a value is truthy, as far as its literals tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    /// Always truthy.
    Truthy,
    /// Always falsy.
    Falsy,
}

impl Truth {
    /// The opposite outcome, for `not`.
    fn negated(self) -> Truth {
        match self {
            Truth::Truthy => Truth::Falsy,
            Truth::Falsy => Truth::Truthy,
        }
    }

    /// `always` or `never`, as the outcome reads in a message.
    fn adverb(self) -> &'static str {
        match self {
            Truth::Truthy => "always",
            Truth::Falsy => "never",
        }
    }

    /// `truthy` or `falsy`.
    fn adjective(self) -> &'static str {
        match self {
            Truth::Truthy => "truthy",
            Truth::Falsy => "falsy",
        }
    }
}

/// The truthiness of `expression` when its literals decide it.
fn truth(expression: &Expression) -> Option<Truth> {
    match &expression.kind {
        ExpressionKind::Nil | ExpressionKind::False => Some(Truth::Falsy),
        ExpressionKind::True
        | ExpressionKind::Number
        | ExpressionKind::String(_)
        | ExpressionKind::Table(_)
        | ExpressionKind::Function(_) => Some(Truth::Truthy),
        ExpressionKind::Parenthesized(inner) => truth(inner),
        ExpressionKind::Unary { operator, operand }
            if matches!(operator.kind, TokenKind::Not | TokenKind::Bang) =>
        {
            truth(operand).map(Truth::negated)
        }
        ExpressionKind::Unary { operator, operand }
            if operator.kind == TokenKind::Minus
                && operand.unparenthesized().kind == ExpressionKind::Number =>
        {
            Some(Truth::Truthy)
        }
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => match operator.kind {
            TokenKind::And | TokenKind::AndAnd => match (truth(left), truth(right)) {
                (Some(Truth::Falsy), _) | (_, Some(Truth::Falsy)) => Some(Truth::Falsy),
                (Some(Truth::Truthy), outcome) => outcome,
                _ => None,
            },
            TokenKind::Or | TokenKind::OrOr => match (truth(left), truth(right)) {
                (Some(Truth::Truthy), _) | (_, Some(Truth::Truthy)) => Some(Truth::Truthy),
                (Some(Truth::Falsy), outcome) => outcome,
                _ => None,
            },
            TokenKind::Coalesce => match left.unparenthesized().kind {
                ExpressionKind::Nil => truth(right),
                _ if is_literal(left) => truth(left),
                _ => None,
            },
            _ => None,
        },
        ExpressionKind::Ternary {
            condition,
            consequent,
            alternative,
            ..
        } => match truth(condition) {
            Some(Truth::Truthy) => truth(consequent),
            Some(Truth::Falsy) => truth(alternative),
            None => truth(consequent).filter(|outcome| truth(alternative) == Some(*outcome)),
        },
        _ => None,
    }
}

/// Whether `expression` is a literal value other than `nil`, which `??` passes on as it is.
fn is_literal(expression: &Expression) -> bool {
    matches!(
        expression.unparenthesized().kind,
        ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Table(_)
            | ExpressionKind::Function(_)
    )
}

/// Collects the decided conditions of a file.
struct Finder<'a> {
    source: &'a Source,
    asserts: Vec<String>,
    found: Vec<(std::ops::Range<usize>, String)>,
}

impl Finder<'_> {
    /// Reports `condition` with `message`.
    fn report(&mut self, condition: &Expression, message: String) {
        self.found.push((condition.range.clone(), message));
    }
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::If { clauses, .. } => {
                for clause in clauses {
                    let Some(outcome) = truth(&clause.condition) else {
                        continue;
                    };
                    let keyword = self.source.text_of(&clause.keyword);
                    self.report(
                        &clause.condition,
                        format!(
                            "Condition is always {}: this `{keyword}` branch {} runs.",
                            outcome.adjective(),
                            outcome.adverb()
                        ),
                    );
                }
            }
            StatementKind::While { condition, .. } if truth(condition) == Some(Truth::Falsy) => {
                self.report(
                    condition,
                    "Condition is always falsy: this `while` loop never runs.".to_owned(),
                );
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Ternary { condition, .. } => {
                if let Some(outcome) = truth(condition) {
                    let branch = match outcome {
                        Truth::Truthy => "first",
                        Truth::Falsy => "second",
                    };
                    self.report(
                        condition,
                        format!(
                            "Condition is always {}: this ternary always gives its {branch} value.",
                            outcome.adjective()
                        ),
                    );
                }
            }
            ExpressionKind::Call { .. } => {
                let Some((name, arguments)) = expression.as_call(self.source) else {
                    return;
                };
                if !self.asserts.contains(&name) {
                    return;
                }
                if let Some(argument) = arguments.values.first()
                    && truth(argument) == Some(Truth::Truthy)
                {
                    self.report(
                        argument,
                        format!("Condition is always truthy: this `{name}` always passes."),
                    );
                }
            }
            _ => {}
        }
    }
}

inventory::submit! { Registration(|| Box::new(LiteralAsCondition)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/LiteralAsCondition";

    #[test]
    fn flags_if_and_elseif_conditions() {
        expect_offenses(
            READER,
            "if true then a() end\nif (false) then a() end\nif x then a() elseif nil then b() end\n\
             if 0 then a() end\nif 'x' then a() end\nif {} then a() end\nif !true then a() end\n\
             if not nil then a() end\nif -1 then a() end\n",
            &[
                (
                    1,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    2,
                    4,
                    "Condition is always falsy: this `if` branch never runs.",
                ),
                (
                    3,
                    22,
                    "Condition is always falsy: this `elseif` branch never runs.",
                ),
                (
                    4,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    5,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    6,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    7,
                    4,
                    "Condition is always falsy: this `if` branch never runs.",
                ),
                (
                    8,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    9,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
            ],
        );
    }

    #[test]
    fn follows_logical_operators() {
        expect_offenses(
            READER,
            "if x or true then a() end\nif x and false then a() end\nif true and false then a() end\n\
             if nil ?? 1 then a() end\nif false ?? x then a() end\nif x ? 1 : 'y' then a() end\n",
            &[
                (
                    1,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    2,
                    4,
                    "Condition is always falsy: this `if` branch never runs.",
                ),
                (
                    3,
                    4,
                    "Condition is always falsy: this `if` branch never runs.",
                ),
                (
                    4,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
                (
                    5,
                    4,
                    "Condition is always falsy: this `if` branch never runs.",
                ),
                (
                    6,
                    4,
                    "Condition is always truthy: this `if` branch always runs.",
                ),
            ],
        );
    }

    #[test]
    fn accepts_conditions_the_literals_do_not_decide() {
        expect_no_offenses(
            READER,
            "if x then a() end\nif x and true then a() end\nif x or false then a() end\n\
             if true and x then a() end\nif false or x then a() end\nif x ?? true then a() end\n\
             if x == true then a() end\nif #t then a() end\nif x ? 1 : nil then a() end\n\
             if -x then a() end\n",
        );
    }

    #[test]
    fn flags_while_loops_that_never_run() {
        expect_offense(
            READER,
            "while false do a() end\nwhile true do a() end\nwhile (true) do a() end\n",
            1,
            7,
            "Condition is always falsy: this `while` loop never runs.",
        );
    }

    #[test]
    fn accepts_repeat_loops() {
        expect_no_offenses(READER, "repeat a() until false\nrepeat a() until true\n");
    }

    #[test]
    fn flags_asserts_that_always_pass() {
        expect_offenses(
            READER,
            "assert(true)\nassert('x', 'message')\nassert(false, 'unreachable')\nassert(x)\n\
             local ok = assert(1)\n",
            &[
                (
                    1,
                    8,
                    "Condition is always truthy: this `assert` always passes.",
                ),
                (
                    2,
                    8,
                    "Condition is always truthy: this `assert` always passes.",
                ),
                (
                    5,
                    19,
                    "Condition is always truthy: this `assert` always passes.",
                ),
            ],
        );
    }

    #[test]
    fn assert_functions_are_configurable() {
        let offenses = inspect_with(
            READER,
            "check(true)\nassert(true)\n",
            "AssertFunctions: [check]",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].line, 1);
    }

    #[test]
    fn flags_ternary_conditions() {
        expect_offenses(
            READER,
            "local a = true ? 1 : 2\nlocal b = nil ? 1 : 2\nlocal c = x ? 1 : 2\n",
            &[
                (
                    1,
                    11,
                    "Condition is always truthy: this ternary always gives its first value.",
                ),
                (
                    2,
                    11,
                    "Condition is always falsy: this ternary always gives its second value.",
                ),
            ],
        );
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(READER, "if true then a() end\nif then\n");
    }
}
