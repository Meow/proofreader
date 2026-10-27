//! `Lint/RemoveInForwardLoop`.

use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags `table.remove(t, i)` inside a loop that walks `t` forward with the index `i`: every
/// removal shifts the following elements down, so the loop skips the element after each removed
/// one.
///
/// The loops are numeric `for i = start, limit do` loops without a step or with a positive
/// numeric literal step whose limit mentions `#t` (`for i = 1, #t do`, `for i = 2, #t - 1, 2
/// do`), and generic `for i, v in ipairs(t) do` and `for k, v in pairs(t) do` loops (`pairs`
/// walks a sequence's keys in the same order and steps past the shifted element the same way).
/// The removal must name the walked table (the same name or field chain of the same variable)
/// and pass exactly the loop variable as the position (a local shadowing it does not count);
/// removals inside functions defined in the loop are ignored. A removal followed in its block,
/// or in an enclosing block inside the loop, by `return`, `goto` or a `break` leaving the walked
/// loop is exempt: it removes one element and stops walking. Files that do not parse are skipped.
pub struct RemoveInForwardLoop;

impl Reader for RemoveInForwardLoop {
    fn name(&self) -> &'static str {
        "Lint/RemoveInForwardLoop"
    }

    fn description(&self) -> &'static str {
        "Checks for `table.remove(t, i)` in a loop walking `t` forward, which skips elements."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let Some(resolution) = resolve(source) else {
            return;
        };
        let mut collector = Collector::default();
        walk(&block, &mut collector);
        let checker = Checker {
            source,
            resolution: &resolution,
            functions: &collector.functions,
        };
        let mut found: Vec<(Range<usize>, String)> = Vec::new();
        for statement in &collector.loops {
            for (range, message) in checker.check(statement, &collector.removals) {
                if !found.iter().any(|(seen, _)| *seen == range) {
                    found.push((range, message));
                }
            }
        }
        found.sort_by_key(|(range, _)| range.start);
        for (range, message) in found {
            ctx.add_offense(range, message);
        }
    }
}

/// The loops, `table.remove` calls and functions of a file, collected in one walk.
#[derive(Default)]
struct Collector<'t> {
    loops: Vec<&'t Statement>,
    removals: Vec<&'t Expression>,
    functions: Vec<Range<usize>>,
}

impl<'t> Visitor<'t> for Collector<'t> {
    fn enter_function(&mut self, function: &'t Function) {
        self.functions
            .push(function.keyword.start..function.end.end);
    }

    fn statement(&mut self, statement: &'t Statement) {
        if matches!(
            statement.kind,
            StatementKind::NumericFor { .. } | StatementKind::GenericFor { .. }
        ) {
            self.loops.push(statement);
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        if let ExpressionKind::Call { callee, arguments } = &expression.kind
            && arguments.values.len() == 2
            && matches!(&callee.kind, ExpressionKind::Field { .. })
        {
            self.removals.push(expression);
        }
    }
}

/// Matches the removals of a file against its loops.
struct Checker<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    functions: &'a [Range<usize>],
}

impl Checker<'_> {
    /// The offenses of the removals in `removals` that `statement`, a loop, walks past.
    fn check(
        &self,
        statement: &Statement,
        removals: &[&Expression],
    ) -> Vec<(Range<usize>, String)> {
        let (variable, body, walked) = match &statement.kind {
            StatementKind::NumericFor {
                variable,
                limit,
                step,
                body,
                ..
            } => {
                if !step.as_ref().is_none_or(|step| self.is_positive(step)) {
                    return Vec::new();
                }
                let mut lengths = Vec::new();
                length_operands(limit, &mut lengths);
                (variable, body, lengths)
            }
            StatementKind::GenericFor {
                variables,
                values,
                body,
            } => {
                let ([variable, ..], [iterator]) = (variables.as_slice(), values.as_slice()) else {
                    return Vec::new();
                };
                let Some((function, arguments)) = iterator.unparenthesized().as_call(self.source)
                else {
                    return Vec::new();
                };
                let [table] = arguments.values.as_slice() else {
                    return Vec::new();
                };
                if !matches!(function.as_str(), "ipairs" | "pairs") {
                    return Vec::new();
                }
                (variable, body, vec![table])
            }
            _ => return Vec::new(),
        };
        let Some(index) = self
            .resolution
            .variables
            .iter()
            .position(|declared| declared.token.start == variable.start)
        else {
            return Vec::new();
        };
        let walked: Vec<Place> = walked
            .iter()
            .filter_map(|table| self.place(table))
            .collect();
        let mut found = Vec::new();
        for removal in removals {
            let ExpressionKind::Call { callee, arguments } = &removal.kind else {
                continue;
            };
            let [table, position] = arguments.values.as_slice() else {
                continue;
            };
            if !contains(&body.range, &removal.range)
                || callee.dotted_name(self.source).as_deref() != Some("table.remove")
                || self.in_nested_function(&body.range, &removal.range)
                || !self.is_variable(position, index)
                || leaves_after(body, removal.range.start, false)
            {
                continue;
            }
            let Some(place) = self.place(table).filter(|place| walked.contains(place)) else {
                continue;
            };
            let name = self.source.text_of(variable);
            found.push((
                removal.range.clone(),
                format!(
                    "Removing from `{0}` while walking it forward skips the element after each \
                     removal; walk it backwards (`for {name} = #{0}, 1, -1 do`).",
                    place.path
                ),
            ));
        }
        found
    }

    /// Whether `step` is a numeric literal greater than zero.
    fn is_positive(&self, step: &Expression) -> bool {
        let step = step.unparenthesized();
        if step.kind != ExpressionKind::Number {
            return false;
        }
        let text = self.source.text[step.range.clone()].trim();
        match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16).is_ok_and(|value| value > 0),
            None => text.parse::<f64>().is_ok_and(|value| value > 0.0),
        }
    }

    /// Whether `range` lies in a function defined inside `body`.
    fn in_nested_function(&self, body: &Range<usize>, range: &Range<usize>) -> bool {
        self.functions
            .iter()
            .any(|function| contains(body, function) && contains(function, range))
    }

    /// Whether `expression` is a read of the variable with index `variable`.
    fn is_variable(&self, expression: &Expression, variable: usize) -> bool {
        let ExpressionKind::Name(token) = &expression.unparenthesized().kind else {
            return false;
        };
        self.resolution
            .reference_at(token.start)
            .is_some_and(|reference| reference.variable == Some(variable))
    }

    /// The place `expression` names, when it is a name or a chain of fields of one.
    fn place(&self, expression: &Expression) -> Option<Place> {
        let expression = expression.unparenthesized();
        let path = expression.dotted_name(self.source)?;
        let root = root_name(expression)?;
        let variable = self
            .resolution
            .reference_at(root.start)
            .and_then(|reference| reference.variable);
        Some(Place { path, variable })
    }
}

/// A table named by a name or field chain: its dotted path and the variable the name at its root
/// resolves to (`None` for a global).
#[derive(Debug, PartialEq, Eq)]
struct Place {
    path: String,
    variable: Option<usize>,
}

/// The name at the root of a name or field chain.
fn root_name(expression: &Expression) -> Option<&Token> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some(token),
        ExpressionKind::Field { object, .. } => root_name(object),
        _ => None,
    }
}

/// Collects the operands of the `#` operators in `expression` (through operators, parentheses
/// and call arguments).
fn length_operands<'t>(expression: &'t Expression, found: &mut Vec<&'t Expression>) {
    match &expression.kind {
        ExpressionKind::Unary { operator, operand } if operator.kind == TokenKind::Hash => {
            found.push(operand);
        }
        ExpressionKind::Unary { operand, .. } => length_operands(operand, found),
        ExpressionKind::Binary { left, right, .. } => {
            length_operands(left, found);
            length_operands(right, found);
        }
        ExpressionKind::Parenthesized(inner) => length_operands(inner, found),
        ExpressionKind::Call { arguments, .. } | ExpressionKind::MethodCall { arguments, .. } => {
            for argument in &arguments.values {
                length_operands(argument, found);
            }
        }
        _ => {}
    }
}

/// Whether `outer` contains all of `inner`.
fn contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Whether the code at byte `offset` in `block` is followed by a statement leaving the walked
/// loop: `return` or `goto` after the statement holding it in its block or an enclosing one, or
/// `break` when no loop nested in the walked one (`nested`) lies in between.
fn leaves_after(block: &Block, offset: usize, nested: bool) -> bool {
    let Some(index) = block
        .statements
        .iter()
        .position(|statement| statement.range.contains(&offset))
    else {
        return false;
    };
    let leaving = block.statements[index + 1..]
        .iter()
        .any(|statement| match statement.kind {
            StatementKind::Return { .. } | StatementKind::Goto { .. } => true,
            StatementKind::Break => !nested,
            _ => false,
        });
    if leaving {
        return true;
    }
    let statement = &block.statements[index];
    let inner = |body: &Block, nested: bool| {
        body.range.contains(&offset) && leaves_after(body, offset, nested)
    };
    match &statement.kind {
        StatementKind::Do { body } => inner(body, nested),
        StatementKind::While { body, .. }
        | StatementKind::Repeat { body, .. }
        | StatementKind::NumericFor { body, .. }
        | StatementKind::GenericFor { body, .. } => inner(body, true),
        StatementKind::If {
            clauses, otherwise, ..
        } => {
            clauses.iter().any(|clause| inner(&clause.body, nested))
                || otherwise
                    .as_ref()
                    .is_some_and(|(_, body)| inner(body, nested))
        }
        _ => false,
    }
}

inventory::submit! { Registration(|| Box::new(RemoveInForwardLoop)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/RemoveInForwardLoop";

    /// The message for removing from `table` in a loop over `variable`.
    fn message(table: &str, variable: &str) -> String {
        format!(
            "Removing from `{table}` while walking it forward skips the element after each \
             removal; walk it backwards (`for {variable} = #{table}, 1, -1 do`)."
        )
    }

    #[test]
    fn flags_removal_in_numeric_loop() {
        expect_offense(
            READER,
            "for i = 1, #t do\n  if t[i].dead then\n    table.remove(t, i)\n  end\nend\n",
            3,
            5,
            &message("t", "i"),
        );
        expect_offense(
            READER,
            "for i = 2, #list - 1, 2 do\n  table.remove(list, i)\nend\n",
            2,
            3,
            &message("list", "i"),
        );
        expect_offense(
            READER,
            "for k = 1, math.min(#self.items, 5) do\n  local x = table.remove(self.items, k)\nend\n",
            2,
            13,
            &message("self.items", "k"),
        );
    }

    #[test]
    fn flags_removal_in_ipairs_and_pairs_loops() {
        expect_offense(
            READER,
            "for i, v in ipairs(t) do\n  if v == x then table.remove(t, i) end\nend\n",
            2,
            18,
            &message("t", "i"),
        );
        expect_offense(
            READER,
            "for k, v in pairs(queue) do\n  table.remove(queue, k)\nend\n",
            2,
            3,
            &message("queue", "k"),
        );
    }

    #[test]
    fn flags_removal_followed_by_continue_or_inner_break() {
        expect_offense(
            READER,
            "for i = 1, #t do\n  if t[i] then\n    table.remove(t, i)\n    continue\n  end\nend\n",
            3,
            5,
            &message("t", "i"),
        );
        expect_offense(
            READER,
            "for i = 1, #t do\n  for j = 1, 3 do\n    table.remove(t, i)\n    break\n  end\nend\n",
            3,
            5,
            &message("t", "i"),
        );
    }

    #[test]
    fn accepts_removal_that_leaves_the_loop() {
        expect_no_offenses(
            READER,
            "for i = 1, #t do\n  if t[i] == x then\n    table.remove(t, i)\n    break\n  end\nend\n\
             for i, v in ipairs(t) do\n  if v == x then\n    table.remove(t, i)\n    return true\n  end\nend\n\
             for i, v in ipairs(t) do\n  if v == x then\n    table.remove(t, i)\n    found = true\n    goto done\n  end\nend\n\
             ::done::\n\
             for i = 1, #t do\n  if t[i] == x then\n    do table.remove(t, i) end\n    break\n  end\nend\n\
             for i = 1, #t do\n  for j = 1, 3 do\n    table.remove(t, i)\n    return\n  end\nend\n",
        );
    }

    #[test]
    fn accepts_backward_loops_and_other_steps() {
        expect_no_offenses(
            READER,
            "for i = #t, 1, -1 do table.remove(t, i) end\n\
             for i = 1, #t, step do table.remove(t, i) end\n\
             for i = 1, #t, 0 do table.remove(t, i) end\n",
        );
    }

    #[test]
    fn accepts_other_tables_and_positions() {
        expect_no_offenses(
            READER,
            "for i = 1, #t do table.remove(other, i) end\n\
             for i = 1, count do table.remove(t, i) end\n\
             for i = 1, #t do table.remove(t, i + 1) end\n\
             for i = 1, #t do table.remove(t) end\n\
             for i = 1, #t do table.remove(t, 1) end\n\
             for i, v in ipairs(t) do table.remove(other, i) end\n\
             for i, v in ipairs(self.a) do table.remove(self.b, i) end\n\
             for i, v in ipairs(t) do table.remove(t, v) end\n\
             for i, v in SortedPairs(t) do table.remove(t, i) end\n\
             for i = 1, #t do foo.remove(t, i) end\n",
        );
    }

    #[test]
    fn requires_the_same_variable() {
        expect_no_offenses(
            READER,
            "for i = 1, #t do\n  local i = 1\n  table.remove(t, i)\nend\n\
             for i = 1, #t do\n  local t = {}\n  table.remove(t, i)\nend\n",
        );
        expect_offense(
            READER,
            "local t = {}\nfor i = 1, #t do\n  table.remove(t, i)\nend\n",
            3,
            3,
            &message("t", "i"),
        );
    }

    #[test]
    fn ignores_removals_in_nested_functions() {
        expect_no_offenses(
            READER,
            "for i = 1, #t do\n  timer.Simple(1, function() table.remove(t, i) end)\nend\n",
        );
    }

    #[test]
    fn reports_each_removal_once() {
        expect_offenses(
            READER,
            "for i = 1, #t do\n  for k, v in ipairs(t) do\n    table.remove(t, i)\n    table.remove(t, k)\n  end\nend\n",
            &[(3, 5, &message("t", "i")), (4, 5, &message("t", "k"))],
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "for i = 1, #t do table.remove(t, i)\n");
    }
}
