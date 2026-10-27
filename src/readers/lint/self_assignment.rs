//! `Lint/SelfAssignment`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::source::Source;

/// Flags assignments of a variable, field or index to itself: `x = x`, `a.b = a.b`,
/// `a[k] = a[k]`, and the targets of a multiple assignment whose value in the same position is
/// the target itself (`a, b = a, c` flags `a`; the swap `a, b = b, a` is fine).
///
/// Both sides must be the same name, or the same field or index chain over a name without calls,
/// whose keys are literals or plain names (`t['x']` and `t.x` are the same); parentheses around
/// the value are ignored. `local x = x`, which caches a global or upvalue in a local, is not an
/// assignment to itself. Files that do not parse are skipped.
pub struct SelfAssignment;

impl Reader for SelfAssignment {
    fn name(&self) -> &'static str {
        "Lint/SelfAssignment"
    }

    fn description(&self) -> &'static str {
        "Checks for variables, fields and indexes assigned to themselves."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(chunk) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            found: Vec::new(),
        };
        walk(&chunk, &mut finder);
        for target in finder.found {
            let text = source.text.get(target.range.clone()).unwrap_or_default();
            ctx.add_offense(
                target.range.clone(),
                format!("`{text}` is assigned to itself."),
            );
        }
    }
}

/// Collects the targets assigned to themselves.
struct Finder<'a, 't> {
    source: &'a Source,
    found: Vec<&'t Expression>,
}

impl<'t> Visitor<'t> for Finder<'_, 't> {
    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::Assignment {
            targets, values, ..
        } = &statement.kind
        else {
            return;
        };
        for (target, value) in targets.iter().zip(values) {
            if let Some(assigned) = path(self.source, target)
                && path(self.source, value.unparenthesized()) == Some(assigned)
            {
                self.found.push(target);
            }
        }
    }
}

/// A canonical spelling of a name, or of a field or index chain over a name whose keys are
/// literals or names (`a.b["c"][1][k]` is `a.b.c[n:1][k]`); `None` for anything else.
fn path(source: &Source, expression: &Expression) -> Option<String> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some(source.text_of(token).to_owned()),
        ExpressionKind::Field { object, name } => Some(format!(
            "{}.{}",
            path(source, object)?,
            source.text_of(name)
        )),
        ExpressionKind::Index { object, key } => {
            let object = path(source, object)?;
            let key = key.unparenthesized();
            Some(match &key.kind {
                ExpressionKind::String(_) => {
                    let contents = key.string_contents(source)?;
                    if is_identifier(contents) {
                        format!("{object}.{contents}")
                    } else if contents.contains('\\') {
                        return None;
                    } else {
                        format!("{object}[s:{contents}]")
                    }
                }
                ExpressionKind::Number => format!("{object}[n:{}]", number(source, key)?),
                ExpressionKind::True => format!("{object}[true]"),
                ExpressionKind::False => format!("{object}[false]"),
                ExpressionKind::Name(token) => format!("{object}[{}]", source.text_of(token)),
                _ => return None,
            })
        }
        _ => None,
    }
}

/// The value of a numeric literal, so that `t[1]` and `t[1.0]` compare equal.
fn number(source: &Source, literal: &Expression) -> Option<f64> {
    let text = source.text.get(literal.range.clone())?.to_ascii_lowercase();
    match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok().map(|n| n as f64),
        None => text.parse().ok(),
    }
}

/// Whether `text` is a Lua identifier, so that `t['text']` is the field `t.text`.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

inventory::submit! { Registration(|| Box::new(SelfAssignment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/SelfAssignment";

    #[test]
    fn flags_names_fields_and_indexes() {
        expect_offense(READER, "x = x\n", 1, 1, "`x` is assigned to itself.");
        expect_offense(
            READER,
            "self.health = self.health\n",
            1,
            1,
            "`self.health` is assigned to itself.",
        );
        expect_offense(
            READER,
            "local function f(t, k)\n  t[k] = t[k]\nend\n",
            2,
            3,
            "`t[k]` is assigned to itself.",
        );
        expect_offense(
            READER,
            "a.b[1] = (a.b[1.0])\n",
            1,
            1,
            "`a.b[1]` is assigned to itself.",
        );
        expect_offense(
            READER,
            "t['x'] = t.x\n",
            1,
            1,
            "`t['x']` is assigned to itself.",
        );
        expect_offense(
            READER,
            "t[\"a b\"] = t['a b']\n",
            1,
            1,
            "`t[\"a b\"]` is assigned to itself.",
        );
    }

    #[test]
    fn flags_targets_of_multiple_assignments() {
        expect_offense(READER, "a, b = a, c\n", 1, 1, "`a` is assigned to itself.");
        expect_offenses(
            READER,
            "a, b = a, b\n",
            &[
                (1, 1, "`a` is assigned to itself."),
                (1, 4, "`b` is assigned to itself."),
            ],
        );
    }

    #[test]
    fn accepts_other_assignments() {
        expect_no_offenses(
            READER,
            "local x = x\nlocal t = t\na, b = b, a\nx = y\nt.a = t.b\nt[1] = t[2]\n\
             t[k] = t[j]\nt.a = u.a\nt[f()] = t[f()]\nf().a = f().a\nt.a = t:a()\n\
             t[k + 1] = t[k + 1]\na, b = c, a\nt.a.b = t.a\n",
        );
    }

    #[test]
    fn checks_nested_functions_and_lumen() {
        expect_offense_lumen(
            READER,
            "return <view>{f(function() v = v end)}</view>\n",
            1,
            28,
            "`v` is assigned to itself.",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "x = x\nf(\n");
    }
}
