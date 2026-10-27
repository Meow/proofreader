//! `Performance/DivisionByConstant`.

use std::ops::Range;

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::readers::performance::hot_paths::is_constant;
use crate::source::Source;
use crate::token::TokenKind;

/// The largest divisor (and inverse of the smallest fractional divisor) flagged by default.
const DEFAULT_MAX_DIVISOR: i64 = 256;

/// Flags division by a numeric literal that is a power of two (`x / 2`, `x / 0.25`, `x / -4`),
/// which multiplication by its reciprocal computes exactly and faster: `x * 0.5`, `x * 4`,
/// `x * -0.25`. Divisors whose reciprocal is not exact (`/ 3`, `/ 10`) are left alone, and so are
/// divisors above `MaxDivisor` and fractional ones below its inverse, whose reciprocals get long.
/// Divisions of a constant (`255 / 256`), which Lua folds when it compiles the file, and hex
/// divisors (`code / 0x40`), which read as bit shifts, are left alone too. The fix replaces the `/` and the literal. Files that do not parse are skipped.
pub struct DivisionByConstant;

impl Reader for DivisionByConstant {
    fn name(&self) -> &'static str {
        "Performance/DivisionByConstant"
    }

    fn description(&self) -> &'static str {
        "Checks for division by a power of two, which multiplying by its reciprocal does exactly and faster."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("MaxDivisor", Yaml::Integer(DEFAULT_MAX_DIVISOR))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let max = ctx.option_usize("MaxDivisor", DEFAULT_MAX_DIVISOR as usize);
        let mut finder = Finder {
            source,
            max: u32::try_from(max).unwrap_or(0),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for found in finder.found {
            ctx.add_offense_with_fix(
                found.range,
                found.message,
                vec![
                    Edit::replace(found.operator, "*"),
                    Edit::replace(found.literal, found.reciprocal),
                ],
            );
        }
    }
}

/// A division by a power of two.
struct Found {
    /// From the `/` to the end of the divisor.
    range: Range<usize>,
    /// The `/`.
    operator: Range<usize>,
    /// The numeric literal of the divisor.
    literal: Range<usize>,
    /// The literal's reciprocal.
    reciprocal: String,
    message: String,
}

/// Collects the divisions by a power of two.
struct Finder<'a> {
    source: &'a Source,
    max: u32,
    found: Vec<Found>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Binary {
            operator,
            left,
            right,
        } = &expression.kind
        else {
            return;
        };
        if operator.kind != TokenKind::Slash || is_constant(left) {
            return;
        }
        let mut divisor = right.unparenthesized();
        if let ExpressionKind::Unary {
            operator: sign,
            operand,
        } = &divisor.kind
            && sign.kind == TokenKind::Minus
        {
            divisor = operand.unparenthesized();
        }
        if divisor.kind != ExpressionKind::Number {
            return;
        }
        let literal = &self.source.text[divisor.range.clone()];
        let Some(reciprocal) = reciprocal(literal, self.max) else {
            return;
        };
        let divisor_text = &self.source.text[right.range.clone()];
        let multiplier = format!(
            "{}{reciprocal}",
            if divisor_text.trim_start().starts_with('-') {
                "-"
            } else {
                ""
            }
        );
        self.found.push(Found {
            range: operator.start..right.range.end,
            operator: operator.range(),
            literal: divisor.range.clone(),
            message: format!(
                "Multiply by `{multiplier}` instead of dividing by `{}`.",
                divisor_text.replace(['(', ')', ' '], "")
            ),
            reciprocal,
        });
    }
}

/// The reciprocal of the decimal literal `literal` when it is a power of two `2^k` with
/// `1 <= |k|` and `2^|k| <= max`, written as a short literal (`2` to `0.5`, `0.25` to `4`).
/// Hex literals (`0x40`), which do not parse as decimals, give `None`.
fn reciprocal(literal: &str, max: u32) -> Option<String> {
    let value = literal.parse::<f64>().ok()?;
    if !value.is_normal() || value <= 0.0 || value == 1.0 {
        return None;
    }
    // A power of two has an all-zero mantissa.
    if value.to_bits() & ((1 << 52) - 1) != 0 {
        return None;
    }
    let inverse = value.recip();
    if value.max(inverse) > f64::from(max) {
        return None;
    }
    Some(if inverse.fract() == 0.0 {
        format!("{}", inverse as i64)
    } else {
        format!("{inverse}")
    })
}

inventory::submit! { Registration(|| Box::new(DivisionByConstant)) }

#[cfg(test)]
mod tests {
    use super::reciprocal;
    use crate::testing::*;

    const READER: &str = "Performance/DivisionByConstant";

    #[test]
    fn finds_exact_reciprocals() {
        assert_eq!(reciprocal("2", 256).as_deref(), Some("0.5"));
        assert_eq!(reciprocal("4.0", 256).as_deref(), Some("0.25"));
        assert_eq!(reciprocal("0x10", 256), None);
        assert_eq!(reciprocal("256", 256).as_deref(), Some("0.00390625"));
        assert_eq!(reciprocal("0.5", 256).as_deref(), Some("2"));
        assert_eq!(reciprocal(".125", 256).as_deref(), Some("8"));
        assert_eq!(reciprocal("2e1", 256), None);
        assert_eq!(reciprocal("1.6e1", 256).as_deref(), Some("0.0625"));
        for literal in [
            "1",
            "3",
            "10",
            "0.2",
            "0",
            "512",
            "0.001953125",
            "2LL",
            "1e400",
        ] {
            assert_eq!(reciprocal(literal, 256), None, "{literal}");
        }
        assert_eq!(reciprocal("512", 1024).as_deref(), Some("0.001953125"));
    }

    #[test]
    fn flags_division_by_powers_of_two() {
        expect_offense(
            READER,
            "local half = w / 2\n",
            1,
            16,
            "Multiply by `0.5` instead of dividing by `2`.",
        );
        expect_offense(
            READER,
            "x = y / -4\n",
            1,
            7,
            "Multiply by `-0.25` instead of dividing by `-4`.",
        );
        expect_offense(
            READER,
            "x = y / 0.5\n",
            1,
            7,
            "Multiply by `2` instead of dividing by `0.5`.",
        );
    }

    #[test]
    fn corrects_to_multiplication() {
        expect_correction(
            READER,
            "draw(x + w / 2, (a + b) / 4 / 2, y / -(8), z/0.25)\n",
            "draw(x + w * 0.5, (a + b) * 0.25 * 0.5, y * -(0.125), z*4)\n",
        );
    }

    #[test]
    fn ignores_inexact_and_other_divisions() {
        expect_no_offenses(
            READER,
            "x = a / 3\nx = a / 10\nx = a / 1\nx = a / b\nx = a / 1024\nx = 2 / a\n\
             x = a * 2\nx = a // 2\nx = '6 / 2'\nx = 255 / 256\nx = -(1 + 3) / 2\nx = code / 0x40\n",
        );
    }

    #[test]
    fn max_divisor_is_configurable() {
        assert_eq!(
            inspect_with(READER, "x = a / 1024\n", "MaxDivisor: 1024").len(),
            1
        );
        assert!(inspect_with(READER, "x = a / 4\n", "MaxDivisor: 2").is_empty());
    }
}
