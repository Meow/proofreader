//! `Performance/PowerOfTwo`.

use std::collections::HashSet;
use std::ops::Range;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::readers::performance::hot_paths::{abbreviated, is_repeatable};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags squaring with `^ 2` (`x ^ 2`, `self.radius ^ 2.0`), which calls `pow` where multiplying
/// the value by itself is cheaper: the fix writes `x * x`. Only bases that can be written twice
/// are flagged: a name, or fields and indexes over a name whose keys are literals or names. The
/// product is parenthesized where `^` bound tighter than `*` would: as the operand of a unary
/// operator (`-x ^ 2` to `-(x * x)`) and as the right operand of `*`, `/`, `%` and `^`
/// (`a / b ^ 2` to `a / (b * b)`). The fix is left out when a comment lies inside the
/// expression. LuaJIT already compiles `x ^ 2` as a multiplication in traces, so this saves time
/// where code runs in the interpreter. A value whose metatable has `__pow` but not `__mul` breaks
/// under the fix. Files that do not parse are skipped.
pub struct PowerOfTwo;

impl Reader for PowerOfTwo {
    fn name(&self) -> &'static str {
        "Performance/PowerOfTwo"
    }

    fn description(&self) -> &'static str {
        "Checks for `x ^ 2`, which multiplying `x * x` computes without calling `pow`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            enclosed: HashSet::new(),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, message, replacement) in finder.found {
            if source.has_comment_in(&range) {
                ctx.add_offense(range, message);
            } else {
                ctx.add_offense_with_fix(
                    range.clone(),
                    message,
                    vec![Edit::replace(range, replacement)],
                );
            }
        }
    }
}

/// Collects the squares of repeatable bases.
struct Finder<'a> {
    source: &'a Source,
    /// The ranges of the operands whose product would need parentheses.
    enclosed: HashSet<(usize, usize)>,
    found: Vec<(Range<usize>, String, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Unary { operand, .. } => {
                self.enclosed
                    .insert((operand.range.start, operand.range.end));
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                if matches!(
                    operator.kind,
                    TokenKind::Star | TokenKind::Slash | TokenKind::Percent | TokenKind::Caret
                ) {
                    self.enclosed.insert((right.range.start, right.range.end));
                }
                if operator.kind == TokenKind::Caret {
                    self.enclosed.insert((left.range.start, left.range.end));
                    self.square(expression, left, right);
                }
            }
            _ => {}
        }
    }
}

impl Finder<'_> {
    /// Records `expression`, `base ^ exponent`, when it squares a repeatable base.
    fn square(&mut self, expression: &Expression, base: &Expression, exponent: &Expression) {
        if exponent.kind != ExpressionKind::Number
            || !is_two(&self.source.text[exponent.range.clone()])
            || !is_repeatable(base)
        {
            return;
        }
        let range = expression.range.clone();
        let base = &self.source.text[base.range.clone()];
        let product = format!("{base} * {base}");
        let replacement = if self.enclosed.contains(&(range.start, range.end)) {
            format!("({product})")
        } else {
            product
        };
        let message = format!(
            "Use `{shown} * {shown}` instead of `{shown} ^ {}`.",
            &self.source.text[exponent.range.clone()],
            shown = abbreviated(base),
        );
        self.found.push((range, message, replacement));
    }
}

/// Whether the numeric literal `literal` is written as two in decimal (`2`, `2.0`); hex and
/// exponent forms are left alone.
fn is_two(literal: &str) -> bool {
    match literal.strip_prefix('2') {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('.')
            .is_some_and(|zeros| zeros.bytes().all(|byte| byte == b'0')),
        None => false,
    }
}

inventory::submit! { Registration(|| Box::new(PowerOfTwo)) }

#[cfg(test)]
mod tests {
    use super::is_two;
    use crate::testing::*;

    const READER: &str = "Performance/PowerOfTwo";

    #[test]
    fn recognises_two() {
        for literal in ["2", "2.0", "2.", "2.00"] {
            assert!(is_two(literal), "{literal}");
        }
        for literal in ["20", "0x2", "2e0", "2.5", "3", "22", "2..0"] {
            assert!(!is_two(literal), "{literal}");
        }
    }

    #[test]
    fn flags_squares() {
        expect_offense(
            READER,
            "local area = r ^ 2\n",
            1,
            14,
            "Use `r * r` instead of `r ^ 2`.",
        );
        expect_offense(
            READER,
            "x = self.radius^2.0\n",
            1,
            5,
            "Use `self.radius * self.radius` instead of `self.radius ^ 2.0`.",
        );
    }

    #[test]
    fn corrects_bare_where_the_product_binds_the_same() {
        expect_correction(
            READER,
            "a = x ^ 2\nb = x ^ 2 + 1\nc = 1 + x ^ 2\nd = 1 - x ^ 2\ne = x ^ 2 * y\n\
             f = x ^ 2 / y\ng = 's' .. x ^ 2\nh = x ^ 2 .. 's'\ni = x ^ 2 < y\nj = y >= x ^ 2\n\
             k = x ^ 2 == y and x ^ 2 or y\nprint(x ^ 2, t[k] ^ 2)\nl = { x ^ 2, n = x ^ 2 }\n\
             m = t[x ^ 2]\nn = (x ^ 2)\nreturn x ^ 2\n",
            "a = x * x\nb = x * x + 1\nc = 1 + x * x\nd = 1 - x * x\ne = x * x * y\n\
             f = x * x / y\ng = 's' .. x * x\nh = x * x .. 's'\ni = x * x < y\nj = y >= x * x\n\
             k = x * x == y and x * x or y\nprint(x * x, t[k] * t[k])\nl = { x * x, n = x * x }\n\
             m = t[x * x]\nn = (x * x)\nreturn x * x\n",
        );
    }

    #[test]
    fn parenthesizes_where_the_product_would_bind_looser() {
        expect_correction(
            READER,
            "a = y * x ^ 2\nb = y / x ^ 2\nc = y % x ^ 2\nd = y ^ x ^ 2\ne = -x ^ 2\n\
             f = #t ^ 2\ng = not x ^ 2\nh = y / x ^ 2 / z\ni = (x ^ 2) ^ 3\n",
            "a = y * (x * x)\nb = y / (x * x)\nc = y % (x * x)\nd = y ^ (x * x)\ne = -(x * x)\n\
             f = #(t * t)\ng = not (x * x)\nh = y / (x * x) / z\ni = (x * x) ^ 3\n",
        );
    }

    #[test]
    fn ignores_other_powers() {
        expect_no_offenses(
            READER,
            "a = x ^ 3\nb = x ^ y\nc = f() ^ 2\nd = (x + 1) ^ 2\ne = 2 ^ 2\nf = x ^ 2 ^ 3\n\
             g = a:b() ^ 2\nh = t[i + 1] ^ 2\ni = x ^ -2\nj = x ^ (2)\nk = x ^ 0x2\n",
        );
    }

    #[test]
    fn keeps_squares_with_comments() {
        let src = "a = x --[[ base ]] ^ 2\n";
        assert_eq!(inspect(READER, src).len(), 1);
        assert_eq!(autocorrect(READER, src), src);
    }
}
