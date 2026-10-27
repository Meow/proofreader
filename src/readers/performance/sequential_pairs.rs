//! `Performance/SequentialPairs`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, FieldKey, Statement, StatementKind, Table, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Functions returning a sequence (their first result, for `file.Find`).
const DEFAULT_SEQUENTIAL_FUNCTIONS: &[&str] = &[
    "player.GetAll",
    "player.GetHumans",
    "player.GetBots",
    "team.GetPlayers",
    "ents.GetAll",
    "ents.FindByClass",
    "ents.FindByModel",
    "ents.FindByName",
    "ents.FindInBox",
    "ents.FindInSphere",
    "ents.FindInCone",
    "ents.FindInPVS",
    "ents.FindAlongRay",
    "string.Explode",
    "string.Split",
    "string.ToTable",
    "table.GetKeys",
    "table.ClearKeys",
    "file.Find",
];

/// Functions that read a table passed to them without changing its keys, besides `pairs`,
/// `ipairs` and `next`, which always do.
const DEFAULT_SAFE_FUNCTIONS: &[&str] = &[
    "unpack",
    "table.unpack",
    "table.concat",
    "table.sort",
    "table.Count",
    "table.HasValue",
    "table.KeyFromValue",
    "table.Random",
    "table.IsEmpty",
    "table.Copy",
    "table.GetKeys",
    "table.maxn",
    "istable",
    "type",
    "tostring",
    "print",
    "PrintTable",
];

/// Flags `pairs(t)` and `next, t` in generic `for` loops when `t` is known to be a sequence
/// (keys `1..n`, no holes): `ipairs` visits the same entries, in order, and LuaJIT compiles it
/// where it cannot compile `pairs`.
///
/// A sequence is a table constructor holding only positional values that cannot be `nil`
/// (literals, tables, functions, arithmetic, `..`, comparisons), a call of one of the
/// `SequentialFunctions`, or a local declared as one of those and never assigned again whose
/// every other use keeps it a sequence: `#t`, `t[k]` and `t.name` reads, passing it to `pairs`,
/// `ipairs`, `next` or one of the `SafeFunctions`, and appending or removing with `table.insert(t, v)`, `t[#t + 1] = v` and
/// `table.remove(t, ...)` outside the loop. Any other use (`t[k] = v`, `t.name = v`, passing it to
/// another function, storing it elsewhere) may break the sequence, and the loop is left alone.
///
/// The fix writes `ipairs(t)`. Files that do not parse are skipped.
pub struct SequentialPairs;

impl Reader for SequentialPairs {
    fn name(&self) -> &'static str {
        "Performance/SequentialPairs"
    }

    fn description(&self) -> &'static str {
        "Checks for `pairs` loops over sequences, which `ipairs` walks faster and in order."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            (
                "SequentialFunctions",
                yaml_strings(DEFAULT_SEQUENTIAL_FUNCTIONS),
            ),
            ("SafeFunctions", yaml_strings(DEFAULT_SAFE_FUNCTIONS)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let Some(resolution) = resolve(source) else {
            return;
        };
        let mut uses = Uses::new(
            source,
            ctx.option_str_list("SafeFunctions", DEFAULT_SAFE_FUNCTIONS),
        );
        walk(&block, &mut uses);
        let checker = Checker {
            source,
            resolution: &resolution,
            uses: &uses,
            sequential: ctx.option_str_list("SequentialFunctions", DEFAULT_SEQUENTIAL_FUNCTIONS),
        };
        for statement in &uses.loops {
            if let Some((range, message, edit)) = checker.check(statement) {
                match edit {
                    Some(edit) => ctx.add_offense_with_fix(range, message, vec![edit]),
                    None => ctx.add_offense(range, message),
                };
            }
        }
    }
}

/// How the code uses the variable names it mentions, collected in one walk.
struct Uses<'t> {
    source: &'t Source,
    safe: Vec<String>,
    /// Name tokens (by offset) in a position that keeps a sequence a sequence.
    keeping: HashSet<usize>,
    /// Name tokens (by offset) assigned a field or index that may break a sequence.
    breaking: HashSet<usize>,
    /// Name tokens (by offset) of appends and removals, with the offset of the change.
    changes: HashMap<usize, usize>,
    /// `local` statements by the offset of each declared name, with the name's position.
    locals: HashMap<usize, (&'t Statement, usize)>,
    /// Every generic `for` statement.
    loops: Vec<&'t Statement>,
}

impl<'t> Uses<'t> {
    /// Empty uses of `source`, with the functions in `safe` keeping the tables passed to them.
    fn new(source: &'t Source, safe: Vec<String>) -> Self {
        Uses {
            source,
            safe,
            keeping: HashSet::new(),
            breaking: HashSet::new(),
            changes: HashMap::new(),
            locals: HashMap::new(),
            loops: Vec::new(),
        }
    }

    /// The name token of `expression` when it is a plain name.
    fn name(expression: &Expression) -> Option<&Token> {
        match &expression.unparenthesized().kind {
            ExpressionKind::Name(token) => Some(token),
            _ => None,
        }
    }

    /// Whether `key` is `#name + 1` (either order) for the name token `name`.
    fn is_append_key(&self, key: &Expression, name: &Token) -> bool {
        let ExpressionKind::Binary {
            operator,
            left,
            right,
        } = &key.unparenthesized().kind
        else {
            return false;
        };
        let is_length = |operand: &Expression| {
            matches!(
                &operand.unparenthesized().kind,
                ExpressionKind::Unary { operator, operand }
                    if operator.kind == TokenKind::Hash
                        && Self::name(operand)
                            .is_some_and(|other| self.source.text_of(other) == self.source.text_of(name))
            )
        };
        let is_one = |operand: &Expression| {
            operand.unparenthesized().kind == ExpressionKind::Number
                && self.source.text[operand.range.clone()].trim() == "1"
        };
        operator.kind == TokenKind::Plus
            && ((is_length(left) && is_one(right)) || (is_one(left) && is_length(right)))
    }
}

impl<'t> Visitor<'t> for Uses<'t> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Local { names, .. } => {
                for (index, name) in names.iter().enumerate() {
                    self.locals.insert(name.start, (statement, index));
                }
            }
            StatementKind::GenericFor { values, .. } => {
                self.loops.push(statement);
                if let [next, table] = values.as_slice()
                    && next.dotted_name(self.source).as_deref() == Some("next")
                    && let Some(name) = Self::name(table)
                {
                    self.keeping.insert(name.start);
                }
            }
            StatementKind::Assignment { targets, .. } => {
                for target in targets {
                    match &target.kind {
                        ExpressionKind::Index { object, key } => {
                            if let Some(name) = Self::name(object) {
                                if self.is_append_key(key, name) {
                                    self.changes.insert(name.start, target.range.start);
                                } else {
                                    self.breaking.insert(name.start);
                                }
                            }
                        }
                        ExpressionKind::Field { object, .. } => {
                            if let Some(name) = Self::name(object) {
                                self.breaking.insert(name.start);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Unary { operator, operand } if operator.kind == TokenKind::Hash => {
                if let Some(name) = Self::name(operand) {
                    self.keeping.insert(name.start);
                }
            }
            ExpressionKind::Index { object, .. } | ExpressionKind::Field { object, .. } => {
                if let Some(name) = Self::name(object) {
                    self.keeping.insert(name.start);
                }
            }
            ExpressionKind::Call { .. } => {
                let Some((function, arguments)) = expression.as_call(self.source) else {
                    return;
                };
                let first = arguments.values.first().and_then(Self::name);
                match (function.as_str(), first) {
                    ("table.insert", Some(name)) if arguments.values.len() == 2 => {
                        self.changes.insert(name.start, expression.range.start);
                    }
                    ("table.remove", Some(name)) => {
                        self.changes.insert(name.start, expression.range.start);
                    }
                    _ if matches!(function.as_str(), "pairs" | "ipairs" | "next")
                        || self.safe.contains(&function) =>
                    {
                        for name in arguments.values.iter().filter_map(Self::name) {
                            self.keeping.insert(name.start);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Decides which loops walk a sequence.
struct Checker<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    uses: &'a Uses<'a>,
    sequential: Vec<String>,
}

impl Checker<'_> {
    /// The offense for a generic `for` loop that walks a sequence with `pairs` or `next`, as
    /// `(range, message, fix)`.
    fn check(&self, statement: &Statement) -> Option<(Range<usize>, String, Option<Edit>)> {
        let StatementKind::GenericFor { values, .. } = &statement.kind else {
            return None;
        };
        let (walker, table, range) = match values.as_slice() {
            [call] => {
                let ExpressionKind::Call { callee, arguments } = &call.unparenthesized().kind
                else {
                    return None;
                };
                let ExpressionKind::Name(walker) = &callee.kind else {
                    return None;
                };
                let [table] = arguments.values.as_slice() else {
                    return None;
                };
                (walker, table, call.range.clone())
            }
            [next, table] => {
                let ExpressionKind::Name(walker) = &next.kind else {
                    return None;
                };
                (walker, table, next.range.start..table.range.end)
            }
            _ => return None,
        };
        let walker_name = self.source.text_of(walker);
        let expected = if values.len() == 1 { "pairs" } else { "next" };
        if walker_name != expected || self.is_local(walker) {
            return None;
        }
        if !self.is_sequence(table, &statement.range) {
            return None;
        }
        let message = format!("Use `ipairs` instead of `{walker_name}` to walk a sequence.");
        let shadowed = self.resolution.visible_at("ipairs", range.start).is_some();
        let edit = if shadowed || self.source.has_comment_in(&range) {
            None
        } else if values.len() == 1 {
            Some(Edit::replace(walker.range(), "ipairs"))
        } else {
            let table = &self.source.text[table.range.clone()];
            Some(Edit::replace(range.clone(), format!("ipairs({table})")))
        };
        Some((range, message, edit))
    }

    /// Whether the name `token` refers to a local rather than the global function.
    fn is_local(&self, token: &Token) -> bool {
        self.resolution
            .reference_at(token.start)
            .is_some_and(|reference| reference.variable.is_some())
    }

    /// Whether `table`, walked by the loop covering `walk`, is known to be a sequence.
    fn is_sequence(&self, table: &Expression, walk: &Range<usize>) -> bool {
        let table = table.unparenthesized();
        if let ExpressionKind::Name(token) = &table.kind {
            return self.is_sequence_local(token, walk);
        }
        self.builds_sequence(table)
    }

    /// Whether `value` builds a sequence: a sequence constructor or a call of a function
    /// returning one.
    fn builds_sequence(&self, value: &Expression) -> bool {
        match &value.unparenthesized().kind {
            ExpressionKind::Table(table) => is_sequence_constructor(table),
            ExpressionKind::Call { .. } => value
                .unparenthesized()
                .as_call(self.source)
                .is_some_and(|(function, _)| self.sequential.contains(&function)),
            _ => false,
        }
    }

    /// Whether the name `token` read by the loop covering `walk` is a local that holds a
    /// sequence for as long as it lives.
    fn is_sequence_local(&self, token: &Token, walk: &Range<usize>) -> bool {
        let Some(id) = self
            .resolution
            .reference_at(token.start)
            .and_then(|reference| reference.variable)
        else {
            return false;
        };
        let variable = &self.resolution.variables[id];
        if variable.kind != DeclarationKind::Local || !variable.writes.is_empty() {
            return false;
        }
        let Some((statement, index)) = self.uses.locals.get(&variable.token.start) else {
            return false;
        };
        let StatementKind::Local { values, .. } = &statement.kind else {
            return false;
        };
        if !values
            .get(*index)
            .is_some_and(|value| self.builds_sequence(value))
        {
            return false;
        }
        self.resolution
            .references
            .iter()
            .filter(|reference| reference.variable == Some(id))
            .all(|reference| {
                let offset = reference.token.start;
                let change = self.uses.changes.get(&offset);
                !self.uses.breaking.contains(&offset)
                    && (self.uses.keeping.contains(&offset) || change.is_some())
                    && change.is_none_or(|at| !walk.contains(at))
            })
    }
}

/// Whether the table constructor `table` builds a sequence: positional fields only, none of
/// which can be `nil`.
fn is_sequence_constructor(table: &Table) -> bool {
    table
        .fields
        .iter()
        .all(|field| field.key == FieldKey::Positional && never_nil(&field.value))
}

/// Whether `expression` produces a value other than `nil` whenever it does not raise an error.
fn never_nil(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Number
        | ExpressionKind::String(_)
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::Table(_)
        | ExpressionKind::Function(_)
        | ExpressionKind::Unary { .. } => true,
        ExpressionKind::Parenthesized(inner) => never_nil(inner),
        ExpressionKind::Binary {
            operator, right, ..
        } => match operator.kind {
            TokenKind::And | TokenKind::AndAnd => false,
            TokenKind::Or | TokenKind::OrOr => never_nil(right),
            _ => true,
        },
        _ => false,
    }
}

inventory::submit! { Registration(|| Box::new(SequentialPairs)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/SequentialPairs";
    const MESSAGE: &str = "Use `ipairs` instead of `pairs` to walk a sequence.";

    #[test]
    fn flags_pairs_over_sequence_constructors_and_functions() {
        expect_offense(
            READER,
            "for _, v in pairs({ 'a', 'b', 1 + x, {} }) do end\n",
            1,
            13,
            MESSAGE,
        );
        expect_offense(
            READER,
            "for _, word in pairs(string.Explode(' ', text)) do end\n",
            1,
            16,
            MESSAGE,
        );
        expect_offense(
            READER,
            "for _, v in next, { 1, 2 } do end\n",
            1,
            13,
            "Use `ipairs` instead of `next` to walk a sequence.",
        );
    }

    #[test]
    fn flags_pairs_over_sequence_locals() {
        expect_offense(
            READER,
            "local ranks = { 'Newcomer', 'Regular' }\n\
             for i, rank in pairs(ranks) do print(i, rank, #ranks, ranks[1]) end\n",
            2,
            16,
            MESSAGE,
        );
        expect_offenses(
            READER,
            "local found = ents.FindInSphere(pos, 100)\nlocal list = {}\n\
             table.insert(list, 1)\nlist[#list + 1] = 2\ntable.sort(list)\n\
             for _, v in pairs(list) do end\nfor _, e in pairs(found) do end\n",
            &[(6, 13, MESSAGE), (7, 13, MESSAGE)],
        );
    }

    #[test]
    fn corrects_to_ipairs() {
        expect_correction(
            READER,
            "local t = { 1, 2 }\nfor k, v in pairs(t) do end\nfor k, v in next, t do end\n",
            "local t = { 1, 2 }\nfor k, v in ipairs(t) do end\nfor k, v in ipairs(t) do end\n",
        );
    }

    #[test]
    fn ignores_tables_that_may_not_be_sequences() {
        expect_no_offenses(
            READER,
            "for k, v in pairs({ a = 1 }) do end\n\
             for k, v in pairs({ 1, x }) do end\n\
             for k, v in pairs({ 1, f() }) do end\n\
             for k, v in pairs({ 1, nil, 3 }) do end\n\
             for k, v in pairs({ 1, a and b }) do end\n\
             for k, v in pairs(get_list()) do end\n\
             for k, v in pairs(t) do end\n\
             for k, v in ipairs({ 1, 2 }) do end\n",
        );
        expect_no_offenses(
            READER,
            "local a = { 1 }\na.name = 'x'\nfor _, v in pairs(a) do end\n\
             local b = { 1 }\nb[5] = 2\nfor _, v in pairs(b) do end\n\
             local c = { 1 }\nc = other\nfor _, v in pairs(c) do end\n\
             local d = { 1 }\nmutate(d)\nfor _, v in pairs(d) do end\n\
             local e = { 1 }\nlocal alias = e\nfor _, v in pairs(e) do end\nprint(alias)\n\
             local f = { 1 }\nfunction f.go() end\nfor _, v in pairs(f) do end\n\
             local g = { 1 }\ng:method()\nfor _, v in pairs(g) do end\n\
             local h = { 1 }\ntable.insert(h, 1, 2)\nfor _, v in pairs(h) do end\n\
             local i\nfor _, v in pairs(i) do end\n",
        );
    }

    #[test]
    fn ignores_loops_that_change_the_sequence() {
        expect_no_offenses(
            READER,
            "local t = { 1, 2, 3 }\nfor k, v in pairs(t) do table.remove(t, k) end\n",
        );
        expect_no_offenses(
            READER,
            "local t = { 1 }\nfor k, v in pairs(t) do t[#t + 1] = v end\n",
        );
    }

    #[test]
    fn respects_local_pairs_and_ipairs() {
        expect_no_offenses(
            READER,
            "local pairs = my_pairs\nfor k, v in pairs({ 1 }) do end\n",
        );
        let src = "local ipairs = nil\nfor k, v in pairs({ 1 }) do end\n";
        assert_eq!(autocorrect(READER, src), src);
        assert!(!inspect(READER, src)[0].correctable());
    }

    #[test]
    fn functions_are_configurable() {
        let options = "SequentialFunctions: [get_list]\nSafeFunctions: [show]";
        let offenses = inspect_with(
            READER,
            "local t = get_list()\nshow(t)\nfor _, v in pairs(t) do end\n\
             for _, w in pairs(string.Explode(' ', s)) do end\n",
            options,
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].line, 3);
    }
}
