//! `Security/SqlInjection`.

use std::collections::{HashMap, HashSet};

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;
use crate::token::TokenKind;

/// The functions running a query given as their first argument.
const DEFAULT_FUNCTIONS: &[&str] = &["sql.Query", "sql.QueryRow", "sql.QueryValue"];

/// The methods running a query given as their first argument (mysqloo's `db:query(text)`).
const DEFAULT_METHODS: &[&str] = &["query"];

/// The functions whose result is safe to put in a query.
const DEFAULT_ESCAPERS: &[&str] = &[
    "sql.SQLStr",
    "SQLStr",
    "tonumber",
    "math.floor",
    "math.Round",
];

/// The methods whose result is safe to put in a query: mysqloo's `db:escape(text)`, gmsv_pg's
/// `db:escape`, `db:quote` and `db:quote_name`.
const DEFAULT_ESCAPE_METHODS: &[&str] = &["escape", "quote", "quote_name"];

/// How many locals a piece of a query is followed through.
const MAX_LOCAL_DEPTH: usize = 4;

/// Longest piece text quoted in a message; longer pieces are described as "this value".
const MAX_QUOTED_PIECE: usize = 40;

/// Flags SQL queries built from values that are not escaped. The query is the first argument
/// of a call of one of the `Functions` (`sql.Query`) or of a method named in `Methods`
/// (`db:query`); when it is a `..` concatenation or a `string.format`/`Format` call (also
/// `fmt:format(...)`), every piece that is not safe is reported. Safe pieces are string,
/// number and boolean literals, calls of the `Escapers` (`sql.SQLStr`, `tonumber`) or of a
/// function of the file that only returns such values (`local function esc(s) return
/// sql.SQLStr(s) end`), calls of methods named in `EscapeMethods` (`db:escape`), arithmetic
/// the bit operators and `#` (whose result is a number), `a and b or c`, `c ? a : b` and `a ?? b`
/// over safe values, arguments of a literal
/// format string taken by a numeric conversion (`%d`, `%f`, ...), numeric `for` loop
/// variables, and locals assigned once from a safe value. A query that is a local assigned
/// once is followed to its value (`local q = '...' .. name`). A plain literal query, or one
/// that is just a name, parameter or call that cannot be traced, is not flagged. Files that do
/// not parse are skipped.
pub struct SqlInjection;

impl Reader for SqlInjection {
    fn name(&self) -> &'static str {
        "Security/SqlInjection"
    }

    fn description(&self) -> &'static str {
        "Checks for SQL queries built from values that are not escaped."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("Functions", yaml_strings(DEFAULT_FUNCTIONS)),
            ("Methods", yaml_strings(DEFAULT_METHODS)),
            ("Escapers", yaml_strings(DEFAULT_ESCAPERS)),
            ("EscapeMethods", yaml_strings(DEFAULT_ESCAPE_METHODS)),
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
        let mut finder = Finder {
            source,
            functions: ctx.option_str_list("Functions", DEFAULT_FUNCTIONS),
            methods: ctx.option_str_list("Methods", DEFAULT_METHODS),
            queries: Vec::new(),
            declarations: HashMap::new(),
            loop_variables: HashSet::new(),
            named_functions: Vec::new(),
        };
        walk(&block, &mut finder);
        let unwritten = || {
            resolution
                .variables
                .iter()
                .enumerate()
                .filter(|(_, variable)| variable.writes.is_empty())
        };
        let constants = unwritten()
            .filter_map(|(id, variable)| {
                finder
                    .declarations
                    .get(&variable.token.start)
                    .map(|value| (id, *value))
            })
            .collect();
        let numbers = unwritten()
            .filter(|(_, variable)| finder.loop_variables.contains(&variable.token.start))
            .map(|(id, _)| id)
            .collect();
        let mut checker = Checker {
            source,
            resolution: &resolution,
            constants,
            numbers,
            escapers: ctx.option_str_list("Escapers", DEFAULT_ESCAPERS),
            escape_methods: ctx.option_str_list("EscapeMethods", DEFAULT_ESCAPE_METHODS),
        };
        let wrappers: Vec<String> = finder
            .named_functions
            .iter()
            .filter(|(_, function)| checker.returns_safe(function))
            .map(|(name, _)| name.clone())
            .collect();
        checker.escapers.extend(wrappers);
        let mut found = Vec::new();
        for (query, receiver) in finder.queries {
            let advice = checker.advice(receiver);
            for piece in checker.unsafe_pieces(query) {
                found.push((piece.range.clone(), describe(source, piece), advice.clone()));
            }
        }
        found.sort_by_key(|(range, _, _)| range.start);
        found.dedup_by_key(|(range, _, _)| range.clone());
        for (range, piece, advice) in found {
            ctx.add_offense(
                range,
                format!("Escape {piece}{advice} before putting it in a query."),
            );
        }
    }
}

/// How a message names `piece`: its text in backticks when it is short, "this value" otherwise.
fn describe(source: &Source, piece: &Expression) -> String {
    let text = &source.text[piece.range.clone()];
    if text.len() <= MAX_QUOTED_PIECE && !text.contains('\n') {
        format!("`{text}`")
    } else {
        "this value".to_owned()
    }
}

/// Collects the query arguments of the checked calls, with the receiver of a method call, the
/// value of every local declared with one, the variables of numeric `for` loops and the
/// functions defined under a bare name.
struct Finder<'a, 't> {
    source: &'a Source,
    functions: Vec<String>,
    methods: Vec<String>,
    queries: Vec<(&'t Expression, Option<&'t Expression>)>,
    /// The values of locals, by the offset of the declared name.
    declarations: HashMap<usize, &'t Expression>,
    /// The offsets of the variables of numeric `for` loops.
    loop_variables: HashSet<usize>,
    /// `local function f` and `function f` definitions.
    named_functions: Vec<(String, &'t Function)>,
}

impl<'t> Visitor<'t> for Finder<'_, 't> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Local { names, values, .. } => {
                for (name, value) in names.iter().zip(values) {
                    self.declarations.insert(name.start, value);
                }
            }
            StatementKind::NumericFor { variable, .. } => {
                self.loop_variables.insert(variable.start);
            }
            StatementKind::LocalFunction { name, function, .. } => {
                self.named_functions
                    .push((self.source.text_of(name).to_owned(), function));
            }
            StatementKind::Function { name, function } => {
                if let ([single], None) = (name.path.as_slice(), &name.method) {
                    self.named_functions
                        .push((self.source.text_of(single).to_owned(), function));
                }
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Call { callee, arguments } => {
                if callee
                    .dotted_name(self.source)
                    .is_some_and(|name| self.functions.contains(&name))
                    && let Some(query) = arguments.values.first()
                {
                    self.queries.push((query, None));
                }
            }
            ExpressionKind::MethodCall {
                object,
                name,
                arguments,
            } => {
                if self
                    .methods
                    .iter()
                    .any(|method| method == self.source.text_of(name))
                    && let Some(query) = arguments.values.first()
                {
                    self.queries.push((query, Some(object)));
                }
            }
            _ => {}
        }
    }
}

/// Decides which pieces of a query are safe.
struct Checker<'a, 't> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    /// The values of the locals assigned only where they are declared, by variable.
    constants: HashMap<usize, &'t Expression>,
    /// The numeric `for` loop variables that are never assigned.
    numbers: HashSet<usize>,
    escapers: Vec<String>,
    escape_methods: Vec<String>,
}

impl<'t> Checker<'_, 't> {
    /// The local variable the name `expression` refers to.
    fn variable(&self, expression: &Expression) -> Option<usize> {
        let ExpressionKind::Name(token) = &expression.kind else {
            return None;
        };
        self.resolution.reference_at(token.start)?.variable
    }

    /// The value of the local assigned only where it is declared that `expression` names.
    fn local_value(&self, expression: &Expression) -> Option<&'t Expression> {
        self.constants.get(&self.variable(expression)?).copied()
    }

    /// Whether every `return` of `function` (outside the functions nested in it) returns one
    /// safe value, and there is at least one: an escaping wrapper such as
    /// `local function esc(s) return sql.SQLStr(s) end`.
    fn returns_safe(&self, function: &Function) -> bool {
        let mut returns = Returns {
            depth: 0,
            values: Vec::new(),
        };
        walk(&function.body, &mut returns);
        !returns.values.is_empty()
            && returns.values.iter().all(|values| match values {
                [value] => self.is_safe(value, 0),
                _ => false,
            })
    }

    /// The escaping to suggest, starting with a space: `db:escape` for a method call on `db`,
    /// the first of the `Escapers` otherwise.
    fn advice(&self, receiver: Option<&Expression>) -> String {
        let method = receiver
            .zip(self.escape_methods.first())
            .and_then(|(object, method)| {
                let text = &self.source.text[object.range.clone()];
                (text.len() <= MAX_QUOTED_PIECE && !text.contains('\n'))
                    .then(|| format!(" with `{text}:{method}`"))
            });
        method
            .or_else(|| {
                self.escapers
                    .first()
                    .map(|escaper| format!(" with `{escaper}`"))
            })
            .unwrap_or_default()
    }

    /// The pieces of `query` that are not safe, when it is built by concatenation or
    /// formatting, directly or in a local assigned once.
    fn unsafe_pieces(&self, query: &'t Expression) -> Vec<&'t Expression> {
        let query = query.unparenthesized();
        let built = match self.local_value(query) {
            Some(value) => value.unparenthesized(),
            None => query,
        };
        let pieces = if is_concat(built) {
            let mut pieces = Vec::new();
            concat_pieces(built, &mut pieces);
            pieces
        } else if let Some(pieces) = self.format_pieces(built) {
            pieces
        } else {
            return Vec::new();
        };
        pieces
            .into_iter()
            .filter(|piece| !self.is_safe(piece, 0))
            .collect()
    }

    /// The arguments of a `string.format`, `Format` or `fmt:format` call that end up in the
    /// text as strings: those taken by `%s` or `%q`, or every argument when the format is not
    /// a literal.
    fn format_pieces(&self, call: &'t Expression) -> Option<Vec<&'t Expression>> {
        let (format, arguments): (&Expression, Vec<&Expression>) = match &call.kind {
            ExpressionKind::Call { callee, arguments } => {
                let name = callee.dotted_name(self.source)?;
                if name != "string.format" && name != "Format" {
                    return None;
                }
                let (format, rest) = arguments.values.split_first()?;
                (format, rest.iter().collect())
            }
            ExpressionKind::MethodCall {
                object,
                name,
                arguments,
            } if self.source.text_of(name) == "format" => {
                (object.unparenthesized(), arguments.values.iter().collect())
            }
            _ => return None,
        };
        let Some(text) = format.string_contents(self.source) else {
            return Some(arguments);
        };
        let conversions = conversions(text);
        Some(
            arguments
                .into_iter()
                .enumerate()
                .filter(|(index, _)| {
                    conversions
                        .get(*index)
                        .is_some_and(|conversion| !is_numeric_conversion(*conversion))
                })
                .map(|(_, argument)| argument)
                .collect(),
        )
    }

    /// Whether `piece` can safely be put in a query as it is.
    fn is_safe(&self, piece: &Expression, depth: usize) -> bool {
        match &piece.kind {
            ExpressionKind::String(_)
            | ExpressionKind::Number
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Nil => true,
            ExpressionKind::Parenthesized(inner) => self.is_safe(inner, depth),
            ExpressionKind::Call { callee, .. } => callee
                .dotted_name(self.source)
                .is_some_and(|name| self.escapers.contains(&name)),
            ExpressionKind::MethodCall { name, .. } => self
                .escape_methods
                .iter()
                .any(|method| method == self.source.text_of(name)),
            ExpressionKind::Unary { operator, operand } => match operator.kind {
                TokenKind::Hash | TokenKind::Tilde => true,
                TokenKind::Minus => self.is_safe(operand, depth),
                _ => false,
            },
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => match operator.kind {
                TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Percent
                | TokenKind::Caret
                | TokenKind::Ampersand
                | TokenKind::Pipe
                | TokenKind::Tilde
                | TokenKind::Shl
                | TokenKind::Shr
                | TokenKind::Sar => true,
                TokenKind::And | TokenKind::AndAnd => self.is_safe(right, depth),
                TokenKind::Or | TokenKind::OrOr | TokenKind::Coalesce => {
                    self.is_safe(left, depth) && self.is_safe(right, depth)
                }
                TokenKind::Concat => self.is_safe(left, depth) && self.is_safe(right, depth),
                _ => false,
            },
            ExpressionKind::Ternary {
                consequent,
                alternative,
                ..
            } => self.is_safe(consequent, depth) && self.is_safe(alternative, depth),
            ExpressionKind::Name(_) => {
                self.variable(piece)
                    .is_some_and(|id| self.numbers.contains(&id))
                    || (depth < MAX_LOCAL_DEPTH
                        && self
                            .local_value(piece)
                            .is_some_and(|value| self.is_safe(value, depth + 1)))
            }
            _ => false,
        }
    }
}

/// Collects the values of the `return` statements of a function body, skipping the functions
/// nested in it.
struct Returns<'t> {
    depth: usize,
    values: Vec<&'t [Expression]>,
}

impl<'t> Visitor<'t> for Returns<'t> {
    fn enter_function(&mut self, _function: &'t Function) {
        self.depth += 1;
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.depth -= 1;
    }

    fn statement(&mut self, statement: &'t Statement) {
        if let (0, StatementKind::Return { values, .. }) = (self.depth, &statement.kind) {
            self.values.push(values);
        }
    }
}

/// Whether `expression` is a `..` concatenation.
fn is_concat(expression: &Expression) -> bool {
    matches!(&expression.kind, ExpressionKind::Binary { operator, .. } if operator.kind == TokenKind::Concat)
}

/// The operands of the `..` chain `expression`, through parentheses, in order.
fn concat_pieces<'t>(expression: &'t Expression, pieces: &mut Vec<&'t Expression>) {
    let inner = expression.unparenthesized();
    match &inner.kind {
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } if operator.kind == TokenKind::Concat => {
            concat_pieces(left, pieces);
            concat_pieces(right, pieces);
        }
        _ => pieces.push(expression),
    }
}

/// The conversion characters of the `%` specifications in a format string, in order (`%%`
/// excluded); an unfinished specification yields `?`.
fn conversions(format: &str) -> Vec<char> {
    let mut found = Vec::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        let conversion = chars
            .by_ref()
            .find(|c| !matches!(c, '-' | '+' | ' ' | '#' | '.' | '0'..='9'))
            .unwrap_or('?');
        if conversion != '%' {
            found.push(conversion);
        }
    }
    found
}

/// Whether the format conversion `c` only accepts numbers (`%d`, `%5.2f`, `%x`, ...).
fn is_numeric_conversion(c: char) -> bool {
    "dicouxXeEfgGaA".contains(c)
}

inventory::submit! { Registration(|| Box::new(SqlInjection)) }

#[cfg(test)]
mod tests {
    use super::conversions;
    use crate::testing::*;

    const READER: &str = "Security/SqlInjection";

    #[test]
    fn flags_unescaped_concatenations() {
        expect_offenses(
            READER,
            "sql.Query('SELECT * FROM t WHERE name = ' .. name)\n\
             sql.QueryValue(\"SELECT x FROM t WHERE a = '\" .. (ply:Nick() .. \"'\"))\n\
             sql.QueryRow('SELECT * FROM t WHERE id = ' .. sql.SQLStr(id) .. ' AND n = ' .. self.n)\n",
            &[
                (
                    1,
                    46,
                    "Escape `name` with `sql.SQLStr` before putting it in a query.",
                ),
                (
                    2,
                    50,
                    "Escape `ply:Nick()` with `sql.SQLStr` before putting it in a query.",
                ),
                (
                    3,
                    80,
                    "Escape `self.n` with `sql.SQLStr` before putting it in a query.",
                ),
            ],
        );
    }

    #[test]
    fn judges_the_extended_operators() {
        expect_no_offenses(
            READER,
            "sql.Query('SELECT ' .. (flags & 1) .. (c ? 'a' : 'b') .. (tonumber(n) ?? 0) .. ~m)\n\
             const function esc(s) return sql.SQLStr(s) end\nsql.Query('SELECT ' .. esc(name))\n",
        );
        expect_offense(
            READER,
            "sql.Query('SELECT ' .. (c ? name : 'b'))\n",
            1,
            24,
            "Escape `(c ? name : 'b')` with `sql.SQLStr` before putting it in a query.",
        );
    }

    #[test]
    fn flags_unescaped_format_arguments() {
        expect_offenses(
            READER,
            "sql.Query(string.format('SELECT * FROM t WHERE a = %s AND b = %d', a, b))\n\
             sql.Query(Format('UPDATE t SET a = %q', a))\n\
             sql.Query(('DELETE FROM t WHERE a = %5.1f OR b = %s'):format(a, b))\n\
             sql.Query(string.format(fmt, a))\n",
            &[
                (
                    1,
                    68,
                    "Escape `a` with `sql.SQLStr` before putting it in a query.",
                ),
                (
                    2,
                    41,
                    "Escape `a` with `sql.SQLStr` before putting it in a query.",
                ),
                (
                    3,
                    65,
                    "Escape `b` with `sql.SQLStr` before putting it in a query.",
                ),
                (
                    4,
                    30,
                    "Escape `a` with `sql.SQLStr` before putting it in a query.",
                ),
            ],
        );
    }

    #[test]
    fn flags_method_queries_with_their_escaper() {
        expect_offense(
            READER,
            "local q = db:query('SELECT * FROM t WHERE a = ' .. db:escape(a) .. ' OR b = ' .. b)\n",
            1,
            82,
            "Escape `b` with `db:escape` before putting it in a query.",
        );
    }

    #[test]
    fn follows_locals() {
        expect_offense(
            READER,
            "local q = 'SELECT * FROM t WHERE a = ' .. a\nsql.Query(q)\n",
            1,
            43,
            "Escape `a` with `sql.SQLStr` before putting it in a query.",
        );
        expect_no_offenses(
            READER,
            "local name = sql.SQLStr(ply:Nick())\nlocal id = tonumber(x)\nlocal TABLE = 'players'\n\
             local q = 'SELECT * FROM ' .. TABLE .. ' WHERE name = ' .. name .. ' AND id = ' .. id\n\
             sql.Query(q)\n",
        );
        expect_no_offenses(
            READER,
            "local q = 'SELECT * FROM t'\nif x then q = q .. ' WHERE a = ' .. a end\nsql.Query(q)\n",
        );
    }

    #[test]
    fn accepts_safe_queries() {
        expect_no_offenses(
            READER,
            "sql.Query('SELECT * FROM t')\nsql.Query(query)\nsql.Query(build(a))\n\
             sql.Query('SELECT * FROM t LIMIT ' .. (page - 1) * 10 .. ', ' .. #list)\n\
             sql.Query('UPDATE t SET a = ' .. (on and 1 or 0) .. ', b = ' .. math.floor(b))\n\
             sql.Query(string.format('SELECT * FROM t WHERE id = %d', id))\n\
             sql.Query('SELECT * FROM t WHERE a = ' .. sql.SQLStr(a) .. ' AND b = ' .. -1)\n\
             mysql:query('players')\nlocal x = obj:queryAll('a' .. b)\n\
             for i = 1, 3 do db:query('SELECT ' .. i .. ', ' .. db:quote(s) .. SQLStr(t)) end\n",
        );
    }

    #[test]
    fn accepts_escaping_wrappers() {
        expect_offense(
            READER,
            "local function esc(s)\n  if !s then return \"''\" end\n  return sql.SQLStr(s)\nend\n\
             local function lower(s) return s:lower() end\n\
             sql.Query(string.format('SELECT %s, %s', esc(a), lower(b)))\n",
            6,
            50,
            "Escape `lower(b)` with `sql.SQLStr` before putting it in a query.",
        );
    }

    #[test]
    fn numeric_loop_variables_must_stay_numbers() {
        expect_offense(
            READER,
            "for i = 1, 3 do\n  i = names[i]\n  sql.Query('SELECT ' .. i)\nend\n",
            3,
            26,
            "Escape `i` with `sql.SQLStr` before putting it in a query.",
        );
    }

    #[test]
    fn options() {
        let offenses = inspect_with(
            READER,
            "db.Run('SELECT ' .. a .. db.Quote(b) .. c:Esc())\nsql.Query('x' .. a)\n",
            "Functions: [db.Run]\nEscapers: [db.Quote]\nEscapeMethods: [Esc]",
        );
        let messages: Vec<&str> = offenses.iter().map(|o| o.message.as_str()).collect();
        assert_eq!(
            messages,
            vec!["Escape `a` with `db.Quote` before putting it in a query."]
        );
    }

    #[test]
    fn reads_format_conversions() {
        assert_eq!(
            conversions("%s %5.2f %% %-3d %q %"),
            vec!['s', 'f', 'd', 'q', '?']
        );
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(READER, "sql.Query('x' .. a)\nif then\n");
    }
}
