//! `Performance/TypeComparison`.

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;
use crate::token::TokenKind;

/// The type names `type` returns and the predicates testing for them directly.
const DEFAULT_PREDICATES: &[(&str, &str)] = &[
    ("number", "isnumber"),
    ("string", "isstring"),
    ("boolean", "isbool"),
    ("table", "istable"),
    ("function", "isfunction"),
    ("Vector", "isvector"),
    ("Angle", "isangle"),
    ("Panel", "ispanel"),
    ("VMatrix", "ismatrix"),
];

/// Longest argument text quoted in a message; longer arguments are shown as `...`.
const MAX_QUOTED_ARGUMENT: usize = 40;

/// Flags `type(x) == 'number'` and `type(x) != 'number'` (either order, `~=` too) when
/// `Predicates` maps the type name to a predicate (`isnumber`), which answers the question
/// without building and comparing a string. The fix writes `isnumber(x)` or
/// `not isnumber(x)`. Files that do not parse are skipped.
pub struct TypeComparison;

impl Reader for TypeComparison {
    fn name(&self) -> &'static str {
        "Performance/TypeComparison"
    }

    fn description(&self) -> &'static str {
        "Checks for `type(x)` compared with a type name that a predicate such as `isnumber` tests directly."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let mut predicates = Hash::new();
        for (type_name, predicate) in DEFAULT_PREDICATES {
            predicates.insert(
                Yaml::String((*type_name).to_owned()),
                Yaml::String((*predicate).to_owned()),
            );
        }
        vec![("Predicates", Yaml::Hash(predicates))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            predicates: ctx.option_str_map("Predicates"),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, message, replacement) in finder.found {
            ctx.add_offense_with_fix(
                range.clone(),
                message,
                vec![Edit::replace(range, replacement)],
            );
        }
    }
}

/// Collects the comparisons of `type(x)` with a type name that has a predicate.
struct Finder<'a> {
    source: &'a Source,
    predicates: Vec<(String, String)>,
    found: Vec<(std::ops::Range<usize>, String, String)>,
}

impl Finder<'_> {
    /// The argument of `operand` when it is `type(x)` with exactly one argument.
    fn type_argument<'t>(&self, operand: &'t Expression) -> Option<&'t Expression> {
        let (function, arguments) = operand.unparenthesized().as_call(self.source)?;
        if function != "type" || arguments.values.len() != 1 {
            return None;
        }
        arguments.values.first()
    }

    /// The predicate for `operand` when it is a string literal naming a type with one.
    fn predicate(&self, operand: &Expression) -> Option<(&str, &str)> {
        let name = operand.unparenthesized().string_contents(self.source)?;
        self.predicates
            .iter()
            .find(|(type_name, _)| type_name == name)
            .map(|(type_name, predicate)| (type_name.as_str(), predicate.as_str()))
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
        if !matches!(operator.kind, TokenKind::Eq | TokenKind::Ne) {
            return;
        }
        let found = match (self.type_argument(left), self.type_argument(right)) {
            (Some(argument), _) => self.predicate(right).map(|found| (argument, found)),
            (None, Some(argument)) => self.predicate(left).map(|found| (argument, found)),
            (None, None) => None,
        };
        let Some((argument, (type_name, predicate))) = found else {
            return;
        };
        let argument = &self.source.text[argument.range.clone()];
        let negated = operator.kind == TokenKind::Ne;
        let replacement = if negated {
            format!("not {predicate}({argument})")
        } else {
            format!("{predicate}({argument})")
        };
        let shown = if argument.len() <= MAX_QUOTED_ARGUMENT && !argument.contains('\n') {
            argument
        } else {
            "..."
        };
        let message = format!(
            "Use `{}{predicate}({shown})` instead of `type({shown}) {} '{type_name}'`.",
            if negated { "not " } else { "" },
            self.source.text_of(operator)
        );
        self.found
            .push((expression.range.clone(), message, replacement));
    }
}

inventory::submit! { Registration(|| Box::new(TypeComparison)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/TypeComparison";

    #[test]
    fn flags_type_comparisons() {
        expect_offense(
            READER,
            "if type(a) == 'number' then end\n",
            1,
            4,
            "Use `isnumber(a)` instead of `type(a) == 'number'`.",
        );
        expect_offense(
            READER,
            "if \"table\" ~= type(self.items) then end\n",
            1,
            4,
            "Use `not istable(self.items)` instead of `type(self.items) ~= 'table'`.",
        );
        expect_offense(
            READER,
            "local ok = (type(x)) != \"Vector\"\n",
            1,
            12,
            "Use `not isvector(x)` instead of `type(x) != 'Vector'`.",
        );
    }

    #[test]
    fn corrects_to_predicates() {
        expect_correction(
            READER,
            "if type(a) == 'number' or type(b) ~= \"string\" then end\n",
            "if isnumber(a) or not isstring(b) then end\n",
        );
        expect_correction(
            READER,
            "return type(f(1, 2)) == 'function' and x\n",
            "return isfunction(f(1, 2)) and x\n",
        );
    }

    #[test]
    fn ignores_other_comparisons() {
        expect_no_offenses(
            READER,
            "if type(a) == 'Entity' then end\nif type(a) == kind then end\nif type(a, b) == 'number' then end\nif type(a) < 'number' then end\nif t.type(a) == 'number' then end\nif type(a) == 'number' .. '' then end\n",
        );
        let offenses = inspect_with(
            READER,
            "if type(a) == 'Entity' then end\nif type(a) == 'number' then end\n",
            "Predicates: { Entity: isentity }",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Use `isentity(a)` instead of `type(a) == 'Entity'`."
        );
    }
}
