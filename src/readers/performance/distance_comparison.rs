//! `Performance/DistanceComparison`.

use std::ops::Range;

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;
use crate::token::TokenKind;

/// The methods taking a square root and the methods returning the squared value instead.
const DEFAULT_METHODS: &[(&str, &str)] = &[
    ("Distance", "DistToSqr"),
    ("Distance2D", "Distance2DSqr"),
    ("Length", "LengthSqr"),
    ("Length2D", "Length2DSqr"),
];

/// The functions taking a square root whose argument can be compared instead.
const DEFAULT_FUNCTIONS: &[&str] = &["math.sqrt"];

/// Longest text of a squared numeric literal written into a fix; beyond it the literal is
/// multiplied by itself instead.
const MAX_SQUARED_LITERAL: usize = 24;

/// Flags a square root whose result is only compared: a call of one of the `Methods`
/// (`pos:Distance(other) < 100`) or `Functions` (`math.sqrt(x) <= y`) standing as an operand of
/// a comparison. The squared method (`DistToSqr`) or the argument of the function compared with
/// the squared bound (`100 * 100`) gives the same answer without the square root. A distance
/// that is part of a larger expression (`a:Distance(b) + c:Distance(d) < 100`) is not flagged.
///
/// The fix squares the call and the bound: a numeric literal becomes its square (`10000`), a
/// name or field is multiplied by itself (`range * range`), anything else is raised to the
/// second power (`f() ^ 2`, `(a + b) ^ 2`); a negated literal bound is not corrected. Both
/// sides of a comparison of two square roots are squared together. Files that do not parse
/// are skipped.
pub struct DistanceComparison;

impl Reader for DistanceComparison {
    fn name(&self) -> &'static str {
        "Performance/DistanceComparison"
    }

    fn description(&self) -> &'static str {
        "Checks for distances and lengths that are only compared, where the squared value does."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let mut methods = Hash::new();
        for (method, squared) in DEFAULT_METHODS {
            methods.insert(
                Yaml::String((*method).to_owned()),
                Yaml::String((*squared).to_owned()),
            );
        }
        vec![
            ("Methods", Yaml::Hash(methods)),
            ("Functions", yaml_strings(DEFAULT_FUNCTIONS)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            methods: ctx.option_str_map("Methods"),
            functions: ctx.option_str_list("Functions", DEFAULT_FUNCTIONS),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, message, fix) in finder.found {
            match fix {
                Some(edits) => ctx.add_offense_with_fix(range, message, edits),
                None => ctx.add_offense(range, message),
            };
        }
    }
}

/// A square root standing as an operand of a comparison.
struct Root<'t> {
    /// The call.
    expression: &'t Expression,
    /// The name as written (`Distance`, `math.sqrt`).
    name: String,
    /// The squared method replacing a method call, or `None` for a function whose argument
    /// replaces the call.
    squared: Option<String>,
}

/// Collects the square roots standing as operands of comparisons.
struct Finder<'a> {
    source: &'a Source,
    methods: Vec<(String, String)>,
    functions: Vec<String>,
    found: Vec<(Range<usize>, String, Option<Vec<Edit>>)>,
}

impl<'t> Finder<'_> {
    /// `operand` as a square root, if it is a call of one of the methods or functions.
    fn root(&self, operand: &'t Expression) -> Option<Root<'t>> {
        let operand = operand.unparenthesized();
        match &operand.kind {
            ExpressionKind::MethodCall { name, .. } => {
                let name = self.source.text_of(name);
                let (_, squared) = self.methods.iter().find(|(method, _)| method == name)?;
                Some(Root {
                    expression: operand,
                    name: name.to_owned(),
                    squared: Some(squared.clone()),
                })
            }
            ExpressionKind::Call { .. } => {
                let (function, arguments) = operand.as_call(self.source)?;
                if !self.functions.contains(&function) || arguments.values.len() != 1 {
                    return None;
                }
                Some(Root {
                    expression: operand,
                    name: function,
                    squared: None,
                })
            }
            _ => None,
        }
    }

    /// The edits turning `root` into its squared value.
    fn square_root(&self, root: &Root<'t>) -> Vec<Edit> {
        match (&root.squared, &root.expression.kind) {
            (Some(squared), ExpressionKind::MethodCall { name, .. }) => {
                vec![Edit::replace(name.range(), squared.clone())]
            }
            (None, ExpressionKind::Call { arguments, .. }) => {
                let argument = &arguments.values[0];
                let text = &self.source.text[argument.range.clone()];
                let replacement = if matches!(argument.kind, ExpressionKind::Binary { .. }) {
                    format!("({text})")
                } else {
                    text.to_owned()
                };
                vec![Edit::replace(root.expression.range.clone(), replacement)]
            }
            _ => Vec::new(),
        }
    }

    /// The edit squaring the bound `bound`, or `None` when it cannot be squared safely.
    fn square_bound(&self, bound: &Expression) -> Option<Edit> {
        let text = &self.source.text[bound.range.clone()];
        let replacement = match &bound.unparenthesized().kind {
            ExpressionKind::Number => {
                let literal = &self.source.text[bound.unparenthesized().range.clone()];
                squared_literal(literal).unwrap_or_else(|| format!("{literal} * {literal}"))
            }
            ExpressionKind::Unary { operand, .. }
                if matches!(operand.unparenthesized().kind, ExpressionKind::Number) =>
            {
                return None;
            }
            ExpressionKind::Name(_)
            | ExpressionKind::Field { .. }
            | ExpressionKind::Index { .. }
                if !has_call(bound) =>
            {
                format!("{text} * {text}")
            }
            ExpressionKind::Binary { .. } | ExpressionKind::Unary { .. }
                if !matches!(bound.kind, ExpressionKind::Parenthesized(_)) =>
            {
                format!("({text}) ^ 2")
            }
            _ => format!("{text} ^ 2"),
        };
        Some(Edit::replace(bound.range.clone(), replacement))
    }

    /// The message for `root` compared with a bound.
    fn message(root: &Root<'t>) -> String {
        match &root.squared {
            Some(squared) => format!(
                "Compare `{squared}` with the squared bound instead of `{}`, which takes a square root.",
                root.name
            ),
            None => format!(
                "Compare the argument of `{}` with the squared bound instead of its result.",
                root.name
            ),
        }
    }
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
        if !matches!(
            operator.kind,
            TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::Eq
                | TokenKind::Ne
        ) {
            return;
        }
        match (self.root(left), self.root(right)) {
            (Some(first), Some(second)) => {
                let mut edits = self.square_root(&first);
                edits.extend(self.square_root(&second));
                let message = format!(
                    "Compare the squared values instead of `{}` and `{}`, which take square roots.",
                    first.name, second.name
                );
                self.found
                    .push((expression.range.clone(), message, Some(edits)));
            }
            (Some(root), None) => self.report(&root, right),
            (None, Some(root)) => self.report(&root, left),
            (None, None) => {}
        }
    }
}

impl<'t> Finder<'_> {
    /// Reports `root` compared with `bound`, with a fix when the bound can be squared.
    fn report(&mut self, root: &Root<'t>, bound: &'t Expression) {
        let fix = self.square_bound(bound).map(|edit| {
            let mut edits = self.square_root(root);
            edits.push(edit);
            edits
        });
        self.found
            .push((root.expression.range.clone(), Self::message(root), fix));
    }
}

/// Whether `expression` contains a call, which squaring by repetition would run twice.
fn has_call(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call { .. } | ExpressionKind::MethodCall { .. } => true,
        ExpressionKind::Field { object, .. } => has_call(object),
        ExpressionKind::Index { object, key } => has_call(object) || has_call(key),
        ExpressionKind::Parenthesized(inner) => has_call(inner),
        _ => false,
    }
}

/// The square of a numeric literal as a literal (`100` to `10000`, `0.5` to `0.25`,
/// `0x10` to `256`), or `None` when it would be long or the literal is not understood.
fn squared_literal(literal: &str) -> Option<String> {
    let lower = literal.to_ascii_lowercase();
    let text = if let Some(hex) = lower.strip_prefix("0x") {
        let value = i128::from_str_radix(hex, 16).ok()?;
        (value * value).to_string()
    } else {
        let value: f64 = lower.parse().ok()?;
        let square = value * value;
        if !square.is_finite() {
            return None;
        }
        if square.fract() == 0.0 && square.abs() < 1e15 {
            format!("{}", square as i64)
        } else {
            format!("{square}")
        }
    };
    (text.len() <= MAX_SQUARED_LITERAL).then_some(text)
}

inventory::submit! { Registration(|| Box::new(DistanceComparison)) }

#[cfg(test)]
mod tests {
    use super::squared_literal;
    use crate::testing::*;

    const READER: &str = "Performance/DistanceComparison";

    #[test]
    fn flags_compared_distances() {
        expect_offense(
            READER,
            "if pos:Distance(target:GetPos()) < 100 then end\n",
            1,
            4,
            "Compare `DistToSqr` with the squared bound instead of `Distance`, which takes a square root.",
        );
        expect_offense(
            READER,
            "local near = range >= (velocity:Length())\n",
            1,
            24,
            "Compare `LengthSqr` with the squared bound instead of `Length`, which takes a square root.",
        );
        expect_offense(
            READER,
            "if math.sqrt(dx * dx + dy * dy) != 0 then end\n",
            1,
            4,
            "Compare the argument of `math.sqrt` with the squared bound instead of its result.",
        );
        expect_offense(
            READER,
            "local closer = a:Distance(b) < c:Distance2D(d)\n",
            1,
            16,
            "Compare the squared values instead of `Distance` and `Distance2D`, which take square roots.",
        );
    }

    #[test]
    fn ignores_distances_that_are_used() {
        expect_no_offenses(
            READER,
            "local d = pos:Distance(other)\nprint(pos:Distance(other) / 2)\nif pos:DistToSqr(other) < 100 * 100 then end\nif d < 100 then end\n",
        );
        expect_no_offenses(
            READER,
            "if a:Distance(b) + c:Distance(d) < 100 then end\nif a:Distance(b) * 2 > limit then end\n",
        );
        expect_no_offenses(READER, "if pos.Distance < 1 then end\n");
        expect_no_offenses(READER, "if math.sqrt(a, b) < 1 then end\n");
    }

    #[test]
    fn squares_literal_bounds() {
        expect_correction(
            READER,
            "if a:Distance(b) == 100 then end\n",
            "if a:DistToSqr(b) == 10000 then end\n",
        );
        expect_correction(
            READER,
            "if 0.5 > a:Length2D() then end\n",
            "if 0.25 > a:Length2DSqr() then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) < 0x10 then end\n",
            "if a:DistToSqr(b) < 256 then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) < 1e3 then end\n",
            "if a:DistToSqr(b) < 1000000 then end\n",
        );
        assert_eq!(squared_literal("1.5"), Some("2.25".to_owned()));
        assert_eq!(squared_literal("1e200"), None);
        assert_eq!(squared_literal("1e-30"), None);
        assert_eq!(squared_literal("2LL"), None);
    }

    #[test]
    fn squares_other_bounds() {
        expect_correction(
            READER,
            "if a:Distance(b) <= range then end\n",
            "if a:DistToSqr(b) <= range * range then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) <= self.range then end\n",
            "if a:DistToSqr(b) <= self.range * self.range then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) > self:GetRange() then end\n",
            "if a:DistToSqr(b) > self:GetRange() ^ 2 then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) > range + 1 then end\n",
            "if a:DistToSqr(b) > (range + 1) ^ 2 then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) > (range + 1) then end\n",
            "if a:DistToSqr(b) > (range + 1) ^ 2 then end\n",
        );
        expect_correction(
            READER,
            "if math.sqrt(dx * dx + dy * dy) < range then end\nif math.sqrt(x) < 3 then end\n",
            "if (dx * dx + dy * dy) < range * range then end\nif x < 9 then end\n",
        );
        expect_correction(
            READER,
            "if a:Distance(b) < c:Distance(d) or math.sqrt(x) == a:Length() then end\n",
            "if a:DistToSqr(b) < c:DistToSqr(d) or x == a:LengthSqr() then end\n",
        );
        let offenses = inspect(READER, "if a:Distance(b) < -1 then end\n");
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
    }

    #[test]
    fn honours_methods_and_functions() {
        let offenses = inspect_with(
            READER,
            "if a:Distance(b) < 1 or vec.len(a) < 1 or a:far(b) < 1 then end\n",
            "Methods: { far: far_sqr }\nFunctions: [vec.len]",
        );
        let messages: Vec<&str> = offenses
            .iter()
            .map(|offense| offense.message.as_str())
            .collect();
        assert_eq!(
            messages,
            vec![
                "Compare the argument of `vec.len` with the squared bound instead of its result.",
                "Compare `far_sqr` with the squared bound instead of `far`, which takes a square root.",
            ]
        );
    }
}
