//! `Lint/NullEntityCheck`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::entities::{Entities, entity_options};
use crate::readers::lint::scopes::resolve;
use crate::token::{Token, TokenKind};

/// Flags an entity tested for `nil` where `IsValid` is needed: a removed entity (or a player
/// who left) is `NULL`, which is truthy and not `nil`, so `if ent then`, `if not ent then`,
/// `ent ~= nil` and `ent == nil` let it through, and the next method call raises "Tried to use
/// a NULL entity!". `IsValid(ent)` is false for both `nil` and `NULL`.
///
/// Flagged are a variable holding an entity standing as a truth value in the condition of an
/// `if`, `elseif`, `while`, `until` or ternary (also under `not`, `!`, `and`, `or` and
/// parentheses), and a comparison of such a variable with `nil` (`==`, `~=`, `!=`) anywhere. A
/// variable holds an entity as for `Lint/UncheckedEntityInCallback` (`EntityTables`,
/// `EntityNames`, `EntityFunctions`, `AlwaysValidFunctions`; see
/// [`crate::readers::lint::entities`]).
///
/// Like `gmod-null-check` of GLuaLS, which proves more with types, a test is only flagged when
/// it matters: the function holding the test (or the file, outside functions) calls a method
/// on the variable after it (`ent:Method()`, `ent?.:Method()`; for a test in the condition of a
/// `repeat` loop, anywhere in the loop) and never checks it with `IsValid(ent)` or
/// `ent:IsValid()`, before or after the test, which covers guards such as `ent and
/// ent:IsValid()` and `if not ent then return end` followed by `if not IsValid(ent) then return
/// end`. Field accesses are not uses, as tables of data share names such as `owner` with
/// entities. A local declared without a value or with `nil` (`local ent`, filled in by a
/// search) is a "not found" marker and never flagged. Files that do not parse are skipped.
///
/// Disabled by default: without types, a name such as `target` or `owner` also holds values
/// that are `nil` or valid, such as the result of a player search or a character object.
///
/// There is no autocorrection: rewriting `ent` to `IsValid(ent)` changes what the condition
/// means for a value that only looks like an entity by its name (a table of player data, a
/// SteamID string), since `IsValid` is false for anything without an `IsValid` method.
pub struct NullEntityCheck;

impl Reader for NullEntityCheck {
    fn name(&self) -> &'static str {
        "Lint/NullEntityCheck"
    }

    fn description(&self) -> &'static str {
        "Checks for entities tested against `nil` instead of with `IsValid`."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        entity_options()
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let Some(resolution) = resolve(source) else {
            return;
        };
        let mut collector = Collector {
            entities: Entities::new(source, ctx),
            functions: Vec::new(),
            file: 0..source.text.len(),
            tests: Vec::new(),
            uses: Vec::new(),
            sentinels: HashSet::new(),
            repeats: Vec::new(),
        };
        walk(&block, &mut collector);
        let variable = |token: &Token| {
            resolution
                .reference_at(token.start)
                .and_then(|reference| reference.variable)
        };
        for test in &collector.tests {
            let Some(id) = variable(&test.name) else {
                continue;
            };
            // A test in the condition of a `repeat` loop runs before the next pass of its
            // body, so the uses in the body count too.
            let after = collector
                .repeats
                .iter()
                .find(|(condition, _)| condition.contains(&test.range.start))
                .map_or(test.range.end, |(_, start)| *start);
            let used_after = collector.uses.iter().any(|token| {
                token.start >= after
                    && test.function.contains(&token.start)
                    && variable(token) == Some(id)
            });
            if !used_after
                || collector
                    .sentinels
                    .contains(&resolution.variables[id].token.start)
                || !collector.entities.is_entity(&resolution, id)
                || collector
                    .entities
                    .checked(&resolution, id, test.function.clone())
            {
                continue;
            }
            let name = source.text_of(&test.name);
            let message = if test.comparison {
                format!(
                    "Comparing `{name}` with `nil` misses NULL entities; check `IsValid({name})` instead."
                )
            } else {
                format!("`{name}` may be NULL, which is truthy; check `IsValid({name})` instead.")
            };
            ctx.add_offense(test.range.clone(), message);
        }
    }
}

/// A test of a variable for `nil`.
struct Test {
    /// The variable name.
    name: Token,
    /// What is reported: the name, or the whole comparison.
    range: Range<usize>,
    /// Whether the test is a comparison with `nil` rather than a truth value.
    comparison: bool,
    /// The function (or the whole file) holding the test.
    function: Range<usize>,
}

/// What the reader needs from one walk over a file.
struct Collector<'t> {
    /// Which variables hold an entity and where they are checked.
    entities: Entities<'t>,
    /// The ranges of the functions being walked, innermost last.
    functions: Vec<Range<usize>>,
    /// The whole file, which holds the code outside functions.
    file: Range<usize>,
    /// Every test of a plain name for `nil`, in source order.
    tests: Vec<Test>,
    /// Every name receiving a method call.
    uses: Vec<Token>,
    /// The name tokens (by offset) of locals declared without a value or with `nil`.
    sentinels: HashSet<usize>,
    /// The condition of every `repeat` loop with the start of the loop.
    repeats: Vec<(Range<usize>, usize)>,
}

impl Collector<'_> {
    /// The innermost function being walked.
    fn function(&self) -> Range<usize> {
        self.functions.last().unwrap_or(&self.file).clone()
    }

    /// Records the names standing as truth values in `condition`.
    fn condition(&mut self, condition: &Expression) {
        match &condition.kind {
            ExpressionKind::Name(token) => {
                self.tests.push(Test {
                    name: *token,
                    range: token.start..token.end,
                    comparison: false,
                    function: self.function(),
                });
            }
            ExpressionKind::Parenthesized(inner) => self.condition(inner),
            ExpressionKind::Unary { operator, operand }
                if matches!(operator.kind, TokenKind::Not | TokenKind::Bang) =>
            {
                self.condition(operand);
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(
                operator.kind,
                TokenKind::And | TokenKind::AndAnd | TokenKind::Or | TokenKind::OrOr
            ) =>
            {
                self.condition(left);
                self.condition(right);
            }
            _ => {}
        }
    }
}

impl<'t> Visitor<'t> for Collector<'t> {
    fn enter_function(&mut self, function: &'t Function) {
        self.functions
            .push(function.keyword.start..function.end.end);
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.functions.pop();
    }

    fn statement(&mut self, statement: &'t Statement) {
        self.entities.statement(statement);
        match &statement.kind {
            StatementKind::Local { names, values, .. } => {
                let multi_valued = values.last().is_some_and(Expression::is_multi_valued);
                for (index, name) in names.iter().enumerate() {
                    let empty = match values.get(index) {
                        Some(value) => value.unparenthesized().kind == ExpressionKind::Nil,
                        None => !multi_valued,
                    };
                    if empty {
                        self.sentinels.insert(name.start);
                    }
                }
            }
            StatementKind::If { clauses, .. } => {
                for clause in clauses {
                    self.condition(&clause.condition);
                }
            }
            StatementKind::While { condition, .. } => self.condition(condition),
            StatementKind::Repeat { condition, .. } => {
                self.repeats
                    .push((condition.range.clone(), statement.range.start));
                self.condition(condition);
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        self.entities.expression(expression);
        match &expression.kind {
            ExpressionKind::Ternary { condition, .. } => self.condition(condition),
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(operator.kind, TokenKind::Eq | TokenKind::Ne) => {
                let (left, right) = (left.unparenthesized(), right.unparenthesized());
                let name = match (&left.kind, &right.kind) {
                    (ExpressionKind::Name(token), ExpressionKind::Nil)
                    | (ExpressionKind::Nil, ExpressionKind::Name(token)) => *token,
                    _ => return,
                };
                self.tests.push(Test {
                    name,
                    range: expression.range.clone(),
                    comparison: true,
                    function: self.function(),
                });
            }
            ExpressionKind::MethodCall { object, .. }
            | ExpressionKind::SafeMethodCall { object, .. } => {
                if let ExpressionKind::Name(token) = &object.unparenthesized().kind {
                    self.uses.push(*token);
                }
            }
            _ => {}
        }
    }
}

inventory::submit! { Registration(|| Box::new(NullEntityCheck)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/NullEntityCheck";

    /// The message for a truth test of `name`.
    fn truthy(name: &str) -> String {
        format!("`{name}` may be NULL, which is truthy; check `IsValid({name})` instead.")
    }

    /// The message for a comparison of `name` with `nil`.
    fn compared(name: &str) -> String {
        format!(
            "Comparing `{name}` with `nil` misses NULL entities; check `IsValid({name})` instead."
        )
    }

    #[test]
    fn flags_truth_tests() {
        expect_offense(
            READER,
            "local function f(ply)\n  if not ply then return end\n  ply:Kill()\nend\n",
            2,
            10,
            &truthy("ply"),
        );
        expect_offense(
            READER,
            "local function f(ent)\n  if ent then\n    ent:Remove()\n  end\nend\n",
            2,
            6,
            &truthy("ent"),
        );
        expect_offense(
            READER,
            "local function f(a, ent)\n  if a and !(ent) then return end\n  print(ent:Health())\nend\n",
            2,
            14,
            &truthy("ent"),
        );
        expect_offense(
            READER,
            "local function f(target)\n  return target ? (target:Nick()) : ''\nend\n",
            2,
            10,
            &truthy("target"),
        );
    }

    #[test]
    fn flags_loop_conditions() {
        expect_offense(
            READER,
            "local function f(ent)\n  while ent do ent = ent:GetParent() end\nend\n",
            2,
            9,
            &truthy("ent"),
        );
        expect_offense(
            READER,
            "local function f(ent)\n  repeat ent = ent:GetParent() until not ent\nend\n",
            2,
            42,
            &truthy("ent"),
        );
    }

    #[test]
    fn flags_comparisons_with_nil() {
        expect_offenses(
            READER,
            "local function f(ent, ply)\n  if ent ~= nil then ent:Remove() end\n\
             if nil == ply then return end\n  ply:Kill()\nend\n",
            &[(2, 6, &compared("ent")), (3, 4, &compared("ply"))],
        );
        expect_offense(
            READER,
            "local function f(ent)\n  local ok = (ent) != nil\n  ent:Remove()\nend\n",
            2,
            14,
            &compared("ent"),
        );
    }

    #[test]
    fn flags_entity_methods_and_entity_locals() {
        expect_offense(
            READER,
            "function ENT:Think()\n  if self then self:NextThink(CurTime()) end\nend\n",
            2,
            6,
            &truthy("self"),
        );
        expect_offense(
            READER,
            "local thing = ents.Create('prop_physics')\nif not thing then return end\nthing:Spawn()\n",
            2,
            8,
            &truthy("thing"),
        );
    }

    #[test]
    fn accepts_validity_checks() {
        expect_no_offenses(
            READER,
            "local function f(ent)\n  if ent and ent:IsValid() then ent:Remove() end\nend\n\
             local function g(ent)\n  if not ent then return end\n  if !IsValid(ent) then return end\n  ent:Remove()\nend\n\
             local function h(ent)\n  if ent ~= nil and IsValid(ent) then ent:Remove() end\nend\n\
             local function i(ent)\n  if IsValid(ent) then ent:Remove() end\nend\n",
        );
    }

    #[test]
    fn accepts_tests_without_a_later_entity_use() {
        expect_no_offenses(
            READER,
            "local function f(ply)\n  ply:Kill()\n  if ply then return true end\nend\n\
             local function g(ent)\n  return ent ~= nil\nend\n\
             local function h(ent)\n  if ent then print(ent) end\nend\n\
             local function i(owner)\n  if !owner then return end\n  return owner.id, owner[1]\nend\n",
        );
    }

    #[test]
    fn accepts_locals_starting_without_a_value() {
        expect_no_offenses(
            READER,
            "local ent\nfor _, v in ipairs(list) do ent = v end\nif ent then ent:Remove() end\n\
             local target, other = nil, 1\nif target then target:Kill() end\n",
        );
        expect_offense(
            READER,
            "local a, ent = f()\nif ent then ent:Remove() end\n",
            2,
            4,
            &truthy("ent"),
        );
    }

    #[test]
    fn uses_in_other_functions_do_not_count() {
        expect_no_offenses(
            READER,
            "local function f(ent)\n  local g = function(ent) return ent:Health() end\n  if ent then return g end\nend\n",
        );
    }

    #[test]
    fn accepts_values_that_are_not_entities() {
        expect_no_offenses(
            READER,
            "local function f(data, client)\n  if data then data:Save() end\nend\n\
             local client = LocalPlayer()\nif client then client:ChatPrint('x') end\n\
             if ply then ply:Kill() end\n\
             local function g(t)\n  if t.ply then t.ply:Kill() end\nend\n",
        );
    }

    #[test]
    fn honours_entity_options() {
        let src = "local function f(actor)\n  if actor then actor:Kill() end\nend\n";
        expect_no_offenses(READER, src);
        let offenses = inspect_with(READER, src, "EntityNames: [actor]");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, truthy("actor"));
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(
            READER,
            "local function f(ent)\n  if ent then ent:Remove() end\nend\nif then\n",
        );
    }
}
