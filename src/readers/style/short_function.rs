//! `Style/ShortFunction`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Function, FunctionForm, StatementKind, Visitor, parse, walk,
};
use crate::readers::style::rewriting::{Placements, Role, has_top_level_method_call, placements};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags anonymous functions that only return one expression (`function(a, b) return a + b
/// end`), which the short functions of LuaJIT 3.0 write as `|a, b| -> a + b` (`a -> a * 2` with
/// one parameter, `|| -> 1` with none). A short function returns every value of its
/// expression, as `return` does, so the fix keeps what the code does; the expression keeps its
/// text.
///
/// Only anonymous functions are flagged (not `function f()` or `local function f()`), whose
/// body is a single `return` of a single expression written on one line and whose parameters
/// end without `...`. A function is left alone where its expression would read differently:
/// as the operand of an operator, the condition of an `if`, loop or ternary, or before a `(`
/// that would call it, in the first branch of a ternary when the expression calls a method, as
/// `a:b` ends that branch there, and before the `:` of a ternary that the expression would take
/// for a method call or for its own ternary. It is also left alone when the line it ends up on
/// would be longer than the `Max` of `Layout/LineLength` (when that reader is enabled).
/// Functions with a comment inside are reported without a fix. Needs a LuaJIT with the syntax
/// extensions of LuaJIT 3.0; disabled by default. Files that do not parse are skipped.
pub struct ShortFunction;

impl Reader for ShortFunction {
    fn name(&self) -> &'static str {
        "Style/ShortFunction"
    }

    fn description(&self) -> &'static str {
        "Checks for anonymous functions that only return an expression, which a short function writes directly."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let max = ctx
            .reader_config("Layout/LineLength")
            .filter(|config| config.enabled)
            .map(|config| config.get_usize("Max", 120));
        let mut finder = Finder {
            source,
            placements: placements(&block),
            max,
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, replacement) in finder.found {
            let message = format!(
                "Use the short function `{}` instead of `function`.",
                replacement.head
            );
            if source.has_comment_in(&range) {
                ctx.add_offense(range, message);
            } else {
                ctx.add_offense_with_fix(
                    range.clone(),
                    message,
                    vec![Edit::replace(range, replacement.text)],
                );
            }
        }
    }
}

/// The short function replacing a function.
struct Replacement {
    /// The parameters and arrow, with `...` for the expression (`|a, b| -> ...`).
    head: String,
    /// The whole short function.
    text: String,
}

/// Collects the functions a short function can replace.
struct Finder<'a> {
    source: &'a Source,
    placements: Placements,
    /// The longest line allowed, if any.
    max: Option<usize>,
    found: Vec<(std::ops::Range<usize>, Replacement)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Function(function) = &expression.kind else {
            return;
        };
        let Some(value) = returned_expression(function) else {
            return;
        };
        let source = self.source;
        let Some(placement) = self.placements.of(expression) else {
            return;
        };
        let readable = !matches!(
            placement.role,
            Role::Operand { .. } | Role::Prefix | Role::Condition | Role::Target | Role::Statement
        );
        let next = source
            .code_tokens()
            .find(|token| token.start >= function.end.end && !token.kind.is_comment())
            .map(|next| next.kind);
        let called = next == Some(TokenKind::LParen);
        let ends_branch = placement.no_method_calls && has_top_level_method_call(source, value);
        let takes_colon = next == Some(TokenKind::TernaryColon)
            && (!placement.no_method_calls || matches!(value.kind, ExpressionKind::Ternary { .. }));
        let body = &source.text[value.range.clone()];
        if !readable || called || ends_branch || takes_colon || body.contains('\n') {
            return;
        }
        let parameters: Vec<&str> = function
            .parameters
            .iter()
            .map(|parameter| source.text_of(parameter))
            .collect();
        let head = match parameters.as_slice() {
            [single] => format!("{single} ->"),
            _ => format!("|{}| ->", parameters.join(", ")),
        };
        let text = format!("{head} {body}");
        if !self.fits(expression, &text) {
            return;
        }
        self.found.push((
            expression.range.clone(),
            Replacement {
                head: format!("{head} ..."),
                text,
            },
        ));
    }
}

impl Finder<'_> {
    /// Whether the lines holding `expression` stay within the maximum length once it is
    /// replaced with `text`.
    fn fits(&self, expression: &Expression, text: &str) -> bool {
        let Some(max) = self.max else {
            return true;
        };
        let source = self.source;
        let (first, _) = source.position(expression.range.start);
        let (last, _) = source.position(expression.range.end.saturating_sub(1));
        let before = &source.text[source.line_range(first).start..expression.range.start];
        let after = &source.text[expression.range.end..source.line_range(last).end];
        format!("{before}{text}{after}")
            .lines()
            .all(|line| line.trim_end_matches('\r').chars().count() <= max)
    }
}

/// The expression `function` returns, when it is a long anonymous function without `...` whose
/// body is a single `return` of a single expression.
fn returned_expression(function: &Function) -> Option<&Expression> {
    if function.form != FunctionForm::Long || function.vararg.is_some() {
        return None;
    }
    let [statement] = function.body.statements.as_slice() else {
        return None;
    };
    match &statement.kind {
        StatementKind::Return { values, .. } => match values.as_slice() {
            [value] => Some(value),
            _ => None,
        },
        _ => None,
    }
}

inventory::submit! { Registration(|| Box::new(ShortFunction)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/ShortFunction";
    const ENABLED: &str = "Enabled: true";

    /// `src` corrected with the reader enabled.
    fn corrected(src: &str) -> String {
        autocorrect_with(READER, src, ENABLED)
    }

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn flags_functions_returning_an_expression() {
        let offenses = inspect_with(READER, "f(function(a, b) return a + b end)\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 3));
        assert_eq!(
            offenses[0].message,
            "Use the short function `|a, b| -> ...` instead of `function`."
        );
    }

    #[test]
    fn writes_every_parameter_list() {
        assert_eq!(
            corrected(
                "f(function(a, b) return a + b end)\ng(function(x) return x * 2 end)\n\
                 h(function() return 1 end)\nlocal k = function(o) return o:name() end\n"
            ),
            "f(|a, b| -> a + b)\ng(x -> x * 2)\nh(|| -> 1)\nlocal k = o -> o:name()\n"
        );
    }

    #[test]
    fn joins_bodies_on_their_own_line() {
        assert_eq!(
            corrected("table.sort(list, function(a, b)\n  return a.points > b.points\nend)\n"),
            "table.sort(list, |a, b| -> a.points > b.points)\n"
        );
        assert_eq!(
            corrected("x = c ? function(o) return o.x end : function() return (f()) end\n"),
            "x = c ? o -> o.x : || -> (f())\n"
        );
        assert_eq!(
            corrected("x = c ? function() return a end : 2\n"),
            "x = c ? || -> a : 2\n"
        );
    }

    #[test]
    fn leaves_functions_that_would_read_differently() {
        for src in [
            "function f(a) return a end\n",
            "local function f(a) return a end\n",
            "f(function(...) return ... end)\n",
            "f(function(a) return a, 1 end)\n",
            "f(function(a) print(a) return a end)\n",
            "f(function(a) return end)\n",
            "x = function() return 1 end + 1\n",
            "if function() return 1 end then end\n",
            "x = (function() return 1 end)(2)\n",
            "x = c ? function(o) return o:m() end : 1\n",
            "f(function(a)\n  return a\n    and b\nend)\n",
            "local f = function() return x end (g)()\n",
            "x = c ? d ? 1 : function() return a end : 2\n",
            "x = c ? function() return a ? b : d end : 5\n",
        ] {
            let found = inspect_with(READER, src, ENABLED);
            let expected = usize::from(src.starts_with("x = (function"));
            assert_eq!(found.len(), expected, "{src}");
        }
        assert_eq!(
            corrected("x = (function() return 1 end)(2)\n"),
            "x = (|| -> 1)(2)\n"
        );
    }

    #[test]
    fn respects_the_line_length() {
        let src = format!("f(function(a) return a + {} end)\n", "b".repeat(100));
        let config = "Style/ShortFunction:\n  Enabled: true\nLayout/LineLength:\n  Max: 100\n";
        assert!(inspect_configured(READER, &src, config).is_empty());
        let loose = "Style/ShortFunction:\n  Enabled: true\nLayout/LineLength:\n  Enabled: false\n";
        assert_eq!(inspect_configured(READER, &src, loose).len(), 1);
    }

    #[test]
    fn reports_functions_with_comments_without_a_fix() {
        let offenses = inspect_with(
            READER,
            "f(function(a) return a -- the value\nend)\n",
            ENABLED,
        );
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }
}
