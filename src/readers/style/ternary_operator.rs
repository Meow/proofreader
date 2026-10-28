//! `Style/TernaryOperator`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::readers::style::rewriting::{Placements, Role, never_false, placements};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags `c and a or b` choosing between two values where the ternary of LuaJIT 3.0 says it
/// directly: the fix writes `c ? a : b`.
///
/// The two only agree when `a` can never be `false` or `nil` (otherwise `c and a or b` gives
/// `b` when `c` holds), so `a` must be a number (`1`, `-1`), a string, a table constructor or
/// `true`. The `or` must be the whole value of a `local`, an assignment, a `return`, a table
/// field, a call argument or a parenthesized expression, never the operand of another operator
/// or a condition, and not in the first branch of another ternary, whose `:` the new ternary
/// would take. The fix only replaces `and` (or `&&`) with `?` and `or` (or `||`) with `:`,
/// so `c`, `a` and `b` keep their text and the line breaks between them stay. Expressions with
/// a comment inside are reported without a fix. Needs a LuaJIT with the syntax extensions of LuaJIT 3.0; disabled by
/// default. Files that do not parse are skipped.
pub struct TernaryOperator;

impl Reader for TernaryOperator {
    fn name(&self) -> &'static str {
        "Style/TernaryOperator"
    }

    fn description(&self) -> &'static str {
        "Checks for `c and a or b` where the ternary `c ? a : b` says the same."
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
            placements: placements(&block),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, edits) in finder.found {
            let message = "Use the ternary `c ? a : b` instead of `c and a or b`.";
            if source.has_comment_in(&range) {
                ctx.add_offense(range, message);
            } else {
                ctx.add_offense_with_fix(range, message, edits);
            }
        }
    }
}

/// Collects the `and`/`or` choices a ternary can replace.
struct Finder<'a> {
    source: &'a Source,
    placements: Placements,
    found: Vec<(std::ops::Range<usize>, Vec<Edit>)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Binary {
            operator: or, left, ..
        } = &expression.kind
        else {
            return;
        };
        let ExpressionKind::Binary {
            operator: and,
            right: consequent,
            ..
        } = &left.kind
        else {
            return;
        };
        if !matches!(or.kind, TokenKind::Or | TokenKind::OrOr)
            || !matches!(and.kind, TokenKind::And | TokenKind::AndAnd)
            || !never_false(consequent)
        {
            return;
        }
        let whole_value = self.placements.of(expression).is_some_and(|placement| {
            !placement.no_method_calls
                && matches!(
                    placement.role,
                    Role::LocalValue
                        | Role::AssignedValue
                        | Role::CompoundValue
                        | Role::ReturnValue
                        | Role::FieldValue
                        | Role::Argument
                        | Role::Parenthesized
                )
        });
        if !whole_value {
            return;
        }
        let edits = vec![self.operator_edit(and, "?"), self.operator_edit(or, ":")];
        self.found.push((expression.range.clone(), edits));
    }
}

impl Finder<'_> {
    /// The edit replacing the operator `token` with `operator`, keeping the line breaks and
    /// spaces around it and adding a space on a side where it touches its operand.
    fn operator_edit(&self, token: &Token, operator: &str) -> Edit {
        let text = &self.source.text;
        let spaced = |byte: Option<&u8>| byte.is_none_or(u8::is_ascii_whitespace);
        let before = if spaced(
            token
                .start
                .checked_sub(1)
                .and_then(|at| text.as_bytes().get(at)),
        ) {
            ""
        } else {
            " "
        };
        let after = if spaced(text.as_bytes().get(token.end)) {
            ""
        } else {
            " "
        };
        Edit::replace(token.range(), format!("{before}{operator}{after}"))
    }
}

inventory::submit! { Registration(|| Box::new(TernaryOperator)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/TernaryOperator";
    const ENABLED: &str = "Enabled: true";
    const MESSAGE: &str = "Use the ternary `c ? a : b` instead of `c and a or b`.";

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
    fn flags_choices_between_values() {
        let offenses = inspect_with(READER, "local x = c and 1 or 2\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 11));
        assert_eq!(offenses[0].message, MESSAGE);
    }

    #[test]
    fn rewrites_in_every_whole_value_position() {
        assert_eq!(
            corrected(
                "local x = c and 'a' or b\ny = c && -1 || f(x)\n\
                 t = { c and true or false, k = a == b and 2 or 3 }\nf(c and 1 or 2, (c and 1 or 2))\n\
                 s ..= c and 'x' or 'y'\ng = || -> c and 1 or 0\nreturn c and {} or nil\n"
            ),
            "local x = c ? 'a' : b\ny = c ? -1 : f(x)\n\
             t = { c ? true : false, k = a == b ? 2 : 3 }\nf(c ? 1 : 2, (c ? 1 : 2))\n\
             s ..= c ? 'x' : 'y'\ng = || -> c ? 1 : 0\nreturn c ? {} : nil\n"
        );
        assert_eq!(
            corrected("x = o:ok() and 'y' or o:name()\nx = a and b and 1 or 2\n"),
            "x = o:ok() ? 'y' : o:name()\nx = a and b ? 1 : 2\n"
        );
        assert_eq!(
            corrected("x = (c and 1 or 2) + 1\nx = 'a' .. (c and 1 or 2)\nx = c and 1 or 2 == 3\n"),
            "x = (c ? 1 : 2) + 1\nx = 'a' .. (c ? 1 : 2)\nx = c ? 1 : 2 == 3\n"
        );
    }

    #[test]
    fn leaves_choices_that_could_differ() {
        assert!(
            inspect_with(
                READER,
                "x = c and a or b\nx = c and false or b\nx = c and nil or b\nx = c and f() or b\n\
                 if c and 1 or 2 then end\n\
                 x = c and 1 or 2 or 3\nx = (c and 1) or 2\nx = c and (1) or 2\n\
                 x = d ? c and 1 or 2 : 3\nwhile c and 1 or nil do end\nx = c and 1 or b ?? 2\n\
                 x = d ? || -> c and 1 or b : y\n",
                ENABLED
            )
            .is_empty()
        );
    }

    #[test]
    fn keeps_line_breaks_and_spaces_operators() {
        assert_eq!(
            corrected("x = c and 'a'\n  or 'b'\ny = c&&.5||2\n"),
            "x = c ? 'a'\n  : 'b'\ny = c ? .5 : 2\n"
        );
    }

    #[test]
    fn reports_choices_with_comments_without_a_fix() {
        let offenses = inspect_with(READER, "x = c and 1 --[[ one ]] or 2\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }
}
