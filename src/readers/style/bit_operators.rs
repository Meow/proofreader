//! `Style/BitOperators`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::style::rewriting::{
    Placements, Role, Side, has_top_level_method_call, placements,
};
use crate::source::Source;
use crate::token::TokenKind;

/// The functions of the `bit` library that have an operator, with the operator and how many
/// arguments a call needs: at least two for the operators that combine any number of values,
/// exactly two for the shifts and exactly one for `bnot`.
const FUNCTIONS: [(&str, &str, Arity); 7] = [
    ("band", "&", Arity::AtLeastTwo),
    ("bor", "|", Arity::AtLeastTwo),
    ("bxor", "~", Arity::AtLeastTwo),
    ("lshift", "<<", Arity::Two),
    ("rshift", ">>", Arity::Two),
    ("arshift", "~>>", Arity::Two),
    ("bnot", "~", Arity::One),
];

/// How many arguments a call of a `bit` function needs to become an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    /// Exactly one.
    One,
    /// Exactly two.
    Two,
    /// Two or more.
    AtLeastTwo,
}

impl Arity {
    /// Whether a call with `count` arguments fits.
    fn accepts(self, count: usize) -> bool {
        match self {
            Arity::One => count == 1,
            Arity::Two => count == 2,
            Arity::AtLeastTwo => count >= 2,
        }
    }
}

/// Flags calls of the `bit` library that the bit operators of LuaJIT 3.0 write directly:
/// `bit.band(a, b, ...)` becomes `a & b & ...`, `bit.bor` `|`, `bit.bxor` `~`,
/// `bit.lshift(a, n)` `a << n`, `bit.rshift` `>>`, `bit.arshift` `~>>` and `bit.bnot(a)` `~a`.
///
/// The operators bind looser than arithmetic and `..`, so the fix parenthesizes the result
/// wherever it is the operand of an operator or is indexed or called (`~a` only as the operand
/// of a unary operator or the base of `^`), and parenthesizes every argument that is a binary
/// operation or a ternary (and an argument of `bnot` that is a unary operation); a unary
/// argument such as `-x` or `~x` binds tighter than every bit operator. Only calls of the
/// global `bit` are flagged (not where a local or parameter named `bit` is in scope), with at
/// least two arguments (exactly two for the shifts, exactly one for `bnot`, whose extra
/// arguments the library ignores). Calls are reported without a fix when their last argument
/// is a call (other than of the `bit` library) or `...`, which passes every value it produces
/// to the library but only the first one to an operator, when an argument is a string literal,
/// when a comment lies inside the call, or when the call stands as a statement. The library converts numeric strings to numbers
/// where the operators raise an error, so a call that only works on a string from elsewhere
/// breaks under the fix. Needs a LuaJIT with the syntax extensions of LuaJIT 3.0; disabled by
/// default. Files that do not parse are skipped.
pub struct BitOperators;

impl Reader for BitOperators {
    fn name(&self) -> &'static str {
        "Style/BitOperators"
    }

    fn description(&self) -> &'static str {
        "Checks for calls of the `bit` library that a bit operator writes directly."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let (Ok(block), Some(resolution)) = (parse(source), resolve(source)) else {
            return;
        };
        let mut finder = Finder {
            source,
            resolution: &resolution,
            placements: placements(&block),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for found in finder.found {
            let message = format!(
                "Use `{}` instead of `bit.{}`.",
                found.operator, found.function
            );
            match found.replacement {
                Some(text) if !source.has_comment_in(&found.range) => {
                    ctx.add_offense_with_fix(
                        found.range.clone(),
                        message,
                        vec![Edit::replace(found.range, text)],
                    );
                }
                _ => {
                    ctx.add_offense(found.range, message);
                }
            }
        }
    }
}

/// A call that an operator can replace.
struct Found<'a> {
    range: std::ops::Range<usize>,
    function: &'a str,
    operator: &'a str,
    /// The operator expression, when the call can be replaced safely.
    replacement: Option<String>,
}

/// Collects the calls of the `bit` library.
struct Finder<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    placements: Placements,
    found: Vec<Found<'a>>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Call { callee, arguments } = &expression.kind else {
            return;
        };
        let ExpressionKind::Field { object, name } = &callee.kind else {
            return;
        };
        let ExpressionKind::Name(library) = &object.kind else {
            return;
        };
        let source = self.source;
        let function = source.text_of(name);
        let Some(&(_, operator, arity)) = FUNCTIONS.iter().find(|(name, ..)| *name == function)
        else {
            return;
        };
        let global = self
            .resolution
            .reference_at(library.start)
            .is_some_and(|reference| reference.variable.is_none());
        let values = &arguments.values;
        if source.text_of(library) != "bit" || !global || !arity.accepts(values.len()) {
            return;
        }
        self.found.push(Found {
            range: expression.range.clone(),
            function,
            operator,
            replacement: self.replacement(expression, values, operator, arity),
        });
    }
}

impl Finder<'_> {
    /// The operator expression replacing the call `expression` with the arguments `values`, or
    /// `None` when it would not compute the same.
    fn replacement(
        &self,
        expression: &Expression,
        values: &[Expression],
        operator: &str,
        arity: Arity,
    ) -> Option<String> {
        let source = self.source;
        let multiple = values.last().is_some_and(|value| {
            value.is_multi_valued()
                && !value
                    .as_call(source)
                    .is_some_and(|(name, _)| name.starts_with("bit."))
        });
        let string = values
            .iter()
            .any(|value| matches!(value.unparenthesized().kind, ExpressionKind::String(_)));
        if multiple || string {
            return None;
        }
        let operands: Vec<String> = values
            .iter()
            .map(|value| {
                let text = &source.text[value.range.clone()];
                let grouped = match &value.kind {
                    ExpressionKind::Binary { .. } | ExpressionKind::Ternary { .. } => true,
                    ExpressionKind::Unary { operator, .. } => {
                        arity == Arity::One
                            || matches!(operator.kind, TokenKind::Not | TokenKind::Bang)
                    }
                    _ => value
                        .as_function()
                        .is_some_and(|function| function.form.is_short()),
                };
                if grouped {
                    format!("({text})")
                } else {
                    text.to_owned()
                }
            })
            .collect();
        let text = if arity == Arity::One {
            format!("{operator}{}", operands[0])
        } else {
            operands.join(&format!(" {operator} "))
        };
        let placement = self.placements.of(expression)?;
        if placement.role == Role::Statement {
            return None;
        }
        let operand = match placement.role {
            Role::Prefix => true,
            Role::Operand { operator, side } => {
                arity != Arity::One
                    || side == Side::Unary
                    || (side == Side::Left && operator == TokenKind::Caret)
            }
            _ => false,
        };
        let enclosed = operand
            || (placement.no_method_calls
                && values
                    .iter()
                    .any(|value| has_top_level_method_call(source, value)));
        Some(if enclosed { format!("({text})") } else { text })
    }
}

inventory::submit! { Registration(|| Box::new(BitOperators)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/BitOperators";
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
    fn flags_library_calls() {
        let offenses = inspect_with(READER, "x = bit.band(a, 1)\ny = bit.bnot(a)\n", ENABLED);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 5, "Use `&` instead of `bit.band`."),
                (2, 5, "Use `~` instead of `bit.bnot`.")
            ]
        );
    }

    #[test]
    fn rewrites_every_function() {
        assert_eq!(
            corrected(
                "x = bit.band(a, b)\nx = bit.bor(a, b, c)\nx = bit.bxor(a, b)\n\
                 x = bit.lshift(a, 2)\nx = bit.rshift(a, 2)\nx = bit.arshift(a, 2)\n\
                 x = bit.bnot(a)\n"
            ),
            "x = a & b\nx = a | b | c\nx = a ~ b\nx = a << 2\nx = a >> 2\nx = a ~>> 2\nx = ~a\n"
        );
    }

    #[test]
    fn parenthesizes_operands_and_results() {
        assert_eq!(
            corrected("x = bit.band(a + 1, -b) == 0\ny = bit.rshift(-1, 28)\nz = bit.bnot(-a)\n"),
            "x = ((a + 1) & -b) == 0\ny = -1 >> 28\nz = ~(-a)\n"
        );
        assert_eq!(
            corrected("x = bit.band(bit.bor(a, b), c)\n"),
            "x = (a | b) & c\n"
        );
        assert_eq!(
            corrected(
                "x = 'f'..bit.lshift(a, 1)\ny = -bit.bnot(a)\nz = bit.band(a, b).x\n\
                 w = bit.band(m, bit.bnot(n)) + bit.bnot(a) ^ 2 + 2 ^ bit.bnot(a)\n"
            ),
            "x = 'f'..(a << 1)\ny = -(~a)\nz = (a & b).x\nw = (m & ~n) + (~a) ^ 2 + 2 ^ ~a\n"
        );
        assert_eq!(
            corrected("x = c ? bit.band(o:m(), 1) : 0\nf(bit.bor(a, 1), { bit.bnot(b) })\n"),
            "x = c ? (o:m() & 1) : 0\nf(a | 1, { ~b })\n"
        );
    }

    #[test]
    fn leaves_unsafe_calls_without_a_fix() {
        for src in [
            "x = bit.band(a, f())\n",
            "x = bit.band(a, ...)\n",
            "x = bit.band('3', 1)\n",
            "x = bit.band(a, --[[ mask ]] 1)\n",
        ] {
            let offenses = inspect_with(READER, src, ENABLED);
            assert_eq!(offenses.len(), 1, "{src}");
            assert!(!offenses[0].correctable(), "{src}");
        }
    }

    #[test]
    fn ignores_other_calls() {
        assert!(
            inspect_with(
                READER,
                "local bit = require('bit32')\nx = bit.band(a, b)\nx = bit.band(a)\n\
                 x = bit.lshift(a, 1, 2)\nx = bit.bnot(a, b)\nx = bit.tohex(a)\n\
                 function f(bit) return bit.bor(a, b) end\n",
                ENABLED
            )
            .is_empty()
        );
        assert_eq!(
            inspect_with(READER, "x = bit.band(a)\ny = bit.band(a, b)\n", ENABLED).len(),
            1
        );
    }
}
