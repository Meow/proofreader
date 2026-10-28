//! `Style/CompoundAssignment`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::performance::hot_paths::abbreviated;
use crate::readers::style::rewriting::{is_simple_target, shape, statement_value_shape};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags assignments that combine a variable with a value and store the result back in it
/// (`x = x + e`), which the compound assignment operators of LuaJIT 3.0 write once
/// (`x += e`). The fix rewrites `x = x op e` to `x op= e` for `+`, `-`, `*`, `/`, `%`, `..`,
/// `&`, `|`, `<<`, `>>` and `~>>`; never for `^` (which has no compound form) or the xor `~`
/// (whose compound form `~=` reads as "not equal").
///
/// Only rewrites that keep what the code does are made: the statement has one target and one
/// value, the value is a binary operation of that operator whose left operand is written
/// exactly like the target (`x = x - a - b` subtracts `x - a` first and is left alone), and the
/// target is a name, a field of a name or an index of a name by a name or a literal (`x`,
/// `t.x`, `t[k]`, `t[1]`), which reads the same whether it is evaluated once or twice. The value
/// keeps its text; parentheses around the whole right operand are dropped when the statement
/// parses the same without them. The spaces between the target and the `=` stay, so the
/// operators of an aligned group stay aligned. Statements with a comment inside are reported
/// without a fix. Needs a LuaJIT with the syntax extensions of LuaJIT 3.0; disabled by default.
/// Files that do not parse are skipped.
pub struct CompoundAssignment;

impl Reader for CompoundAssignment {
    fn name(&self) -> &'static str {
        "Style/CompoundAssignment"
    }

    fn description(&self) -> &'static str {
        "Checks for `x = x + e` where the compound assignment `x += e` says the same."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (statement, operator, replacement) in finder.found {
            let message = format!(
                "Use `{operator}=` instead of repeating `{}`.",
                abbreviated(replacement.target)
            );
            if source.has_comment_in(&statement) {
                ctx.add_offense(statement, message);
            } else {
                let text = replacement.text(operator);
                ctx.add_offense_with_fix(
                    statement.clone(),
                    message,
                    vec![Edit::replace(statement, text)],
                );
            }
        }
    }
}

/// The parts of the compound assignment replacing a statement.
struct Replacement<'a> {
    /// The text of the target.
    target: &'a str,
    /// The spaces between the target and the operator: those before the `=` of the
    /// assignment, which keeps the operator where the `=` of an aligned group was.
    padding: &'a str,
    /// The text of the value.
    value: &'a str,
}

impl Replacement<'_> {
    /// The statement `target op= value`.
    fn text(&self, operator: &str) -> String {
        format!("{}{}{operator}= {}", self.target, self.padding, self.value)
    }
}

/// Collects the assignments that a compound assignment can replace.
struct Finder<'a> {
    source: &'a Source,
    found: Vec<(std::ops::Range<usize>, &'a str, Replacement<'a>)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::Assignment {
            targets,
            assign,
            values,
        } = &statement.kind
        else {
            return;
        };
        let ([target], [value]) = (targets.as_slice(), values.as_slice()) else {
            return;
        };
        let ExpressionKind::Binary {
            operator,
            left,
            right,
        } = &value.kind
        else {
            return;
        };
        if !compounds(operator.kind) || !is_simple_target(target) {
            return;
        }
        let source = self.source;
        let text = |expression: &Expression| &source.text[expression.range.clone()];
        if text(left) != text(target) {
            return;
        }
        let operator = source.text_of(operator);
        let padding = &source.text[target.range.end..assign.start];
        let padding = if padding.is_empty() || padding.contains('\n') {
            " "
        } else {
            padding
        };
        let mut replacement = Replacement {
            target: text(target),
            padding,
            value: text(right),
        };
        if let ExpressionKind::Parenthesized(inner) = &right.kind {
            let bare = Replacement {
                value: text(inner),
                ..replacement
            };
            if statement_value_shape(&bare.text(operator)).as_deref()
                == Some(shape(source, inner).as_str())
            {
                replacement = bare;
            }
        }
        self.found
            .push((statement.range.clone(), operator, replacement));
    }
}

/// Whether the binary operator `kind` has a compound assignment form the reader writes.
fn compounds(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Concat
            | TokenKind::Ampersand
            | TokenKind::Pipe
            | TokenKind::Shl
            | TokenKind::Shr
            | TokenKind::Sar
    )
}

inventory::submit! { Registration(|| Box::new(CompoundAssignment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/CompoundAssignment";

    /// `src` corrected with the reader enabled.
    fn corrected(src: &str) -> String {
        autocorrect_with(READER, src, "Enabled: true")
    }

    /// The offenses for `src` with the reader enabled.
    fn found(src: &str) -> Vec<(u32, u32, String)> {
        inspect_with(READER, src, "Enabled: true")
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect()
    }

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn flags_repeated_targets() {
        assert_eq!(
            found("x = x + 1\nself.count = self.count .. 'a'\nt[k] = t[k] - f(x)\n"),
            vec![
                (1, 1, "Use `+=` instead of repeating `x`.".to_owned()),
                (
                    2,
                    1,
                    "Use `..=` instead of repeating `self.count`.".to_owned()
                ),
                (3, 1, "Use `-=` instead of repeating `t[k]`.".to_owned()),
            ]
        );
    }

    #[test]
    fn rewrites_every_operator() {
        for operator in ["+", "-", "*", "/", "%", "..", "&", "|", "<<", ">>", "~>>"] {
            assert_eq!(
                corrected(&format!("x = x {operator} y\n")),
                format!("x {operator}= y\n"),
                "{operator}"
            );
        }
        assert_eq!(
            corrected("t[1] = t[1] + 1\nt['a'] = t['a'] .. b\n"),
            "t[1] += 1\nt['a'] ..= b\n"
        );
    }

    #[test]
    fn keeps_the_value_and_its_grouping() {
        assert_eq!(corrected("x = x + a * b\n"), "x += a * b\n");
        assert_eq!(corrected("s = s .. a .. b\n"), "s ..= a .. b\n");
        assert_eq!(corrected("x = x * (a + b)\n"), "x *= a + b\n");
        assert_eq!(
            corrected("x = x .. (c ? 'a' : 'b')\n"),
            "x ..= c ? 'a' : 'b'\n"
        );
        assert_eq!(corrected("x = x + (f())\n"), "x += f()\n");
        assert_eq!(corrected("x = x +\n  y\n"), "x += y\n");
    }

    #[test]
    fn keeps_aligned_operators_aligned() {
        assert_eq!(
            corrected("mm = mm - cm * 10\ncm = cm - m  * 100\nm  = m  - km * 1000\nx=x+1\n"),
            "mm -= cm * 10\ncm -= m  * 100\nm  -= km * 1000\nx += 1\n"
        );
    }

    #[test]
    fn leaves_other_assignments_alone() {
        assert!(
            found(
                "x = x - a - b\nx = x ^ 2\nx = x ~ y\nx = y + x\nx = x == y\nx, y = x + 1, y\n\
             local x = x + 1\nt.a.b = t.a.b + 1\nt[f()] = t[f()] + 1\nf().x = f().x + 1\n\
             t[a.b] = t[a.b] + 1\nx = (x) + 1\nx = x + 1 + 2\na?.b = a?.b + 1\nx = x ?? 1\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn reports_statements_with_comments_without_a_fix() {
        let offenses = inspect_with(READER, "x = x + --[[ one ]] 1\n", "Enabled: true");
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }
}
