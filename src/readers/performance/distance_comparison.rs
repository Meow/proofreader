//! `Performance/DistanceComparison`.

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

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

/// Flags a square root whose result is only compared: a call of one of the `Methods`
/// (`pos:Distance(other) < 100`) or `Functions` (`math.sqrt(x) <= y`) standing as an operand of
/// a comparison. The squared method (`DistToSqr`) or the argument of the function compared with
/// the squared bound (`100 * 100`) gives the same answer without the square root. Files that
/// do not parse are skipped.
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
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the square roots standing as operands of comparisons.
struct Finder<'a> {
    source: &'a Source,
    methods: Vec<(String, String)>,
    functions: Vec<String>,
    found: Vec<(std::ops::Range<usize>, String)>,
}

impl Finder<'_> {
    /// Reports `operand` when it is a call taking a square root.
    fn check(&mut self, operand: &Expression) {
        let operand = operand.unparenthesized();
        match &operand.kind {
            ExpressionKind::MethodCall { name, .. } => {
                let name = self.source.text_of(name);
                let Some((_, squared)) = self.methods.iter().find(|(method, _)| method == name)
                else {
                    return;
                };
                self.found.push((
                    operand.range.clone(),
                    format!(
                        "Compare `{squared}` with the squared bound instead of `{name}`, which takes a square root."
                    ),
                ));
            }
            ExpressionKind::Call { .. } => {
                let Some((function, _)) = operand.as_call(self.source) else {
                    return;
                };
                if !self.functions.contains(&function) {
                    return;
                }
                self.found.push((
                    operand.range.clone(),
                    format!(
                        "Compare the argument of `{function}` with the squared bound instead of its result."
                    ),
                ));
            }
            _ => {}
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
        self.check(left);
        self.check(right);
    }
}

inventory::submit! { Registration(|| Box::new(DistanceComparison)) }

#[cfg(test)]
mod tests {
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
        expect_offenses(
            READER,
            "local closer = a:Distance(b) < c:Distance2D(d)\n",
            &[
                (
                    1,
                    16,
                    "Compare `DistToSqr` with the squared bound instead of `Distance`, which takes a square root.",
                ),
                (
                    1,
                    32,
                    "Compare `Distance2DSqr` with the squared bound instead of `Distance2D`, which takes a square root.",
                ),
            ],
        );
    }

    #[test]
    fn ignores_distances_that_are_used() {
        expect_no_offenses(
            READER,
            "local d = pos:Distance(other)\nprint(pos:Distance(other) / 2)\nif pos:DistToSqr(other) < 100 * 100 then end\nif d < 100 then end\n",
        );
        expect_no_offenses(READER, "if pos.Distance < 1 then end\n");
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
