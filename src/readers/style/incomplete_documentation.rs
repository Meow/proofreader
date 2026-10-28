//! `Style/IncompleteDocumentation`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Block, Expression, ExpressionKind, StatementKind, parse};
use crate::readers::style::doc_comments::{LDOC_TYPED_PARAM_TAGS, functions};
use crate::token::Token;

/// Requires a function whose doc comment has a parameter or return tag to document all of its
/// signature: an `@param` for every named parameter (names starting with `_` exempt) and an
/// `@return` (or `@treturn`) when its own body returns a value other than `nil`.
///
/// Parameter tags are those of `Lint/UndefinedDocParam` (`@param`, `@tparam` and the
/// `TypedParamTags` aliases). Functions without tags in their doc comment, or without a doc
/// comment, are left to `Style/DocumentationMethod`. Local functions (`local function f()`,
/// `local f = function()`) are skipped unless `RequireForLocalFunctions` is set. Files that do
/// not parse are skipped. Disabled by default.
pub struct IncompleteDocumentation;

impl Reader for IncompleteDocumentation {
    fn name(&self) -> &'static str {
        "Style/IncompleteDocumentation"
    }

    fn description(&self) -> &'static str {
        "Checks that documented functions document every parameter and their return value."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("RequireForLocalFunctions", Yaml::Boolean(false)),
            (
                "TypedParamTags",
                Yaml::Array(
                    LDOC_TYPED_PARAM_TAGS
                        .iter()
                        .map(|tag| Yaml::String((*tag).to_owned()))
                        .collect(),
                ),
            ),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let require_local = ctx.option_bool("RequireForLocalFunctions", false);
        let typed = ctx.option_str_list("TypedParamTags", &LDOC_TYPED_PARAM_TAGS);
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        for function in functions(source, &block) {
            if function.local && !require_local {
                continue;
            }
            let Some(doc) = &function.doc else {
                continue;
            };
            if !doc.has_signature_tags(&typed) {
                continue;
            }
            let documented = doc.params(&typed);
            for parameter in &function.function.parameters {
                let name = source.text_of(parameter);
                if name.starts_with('_') || documented.iter().any(|param| param.name == name) {
                    continue;
                }
                ctx.add_offense(parameter.range(), format!("Missing `@param` for `{name}`."));
            }
            if !doc.has_return()
                && let Some(keyword) = first_value_return(&function.function.body)
            {
                ctx.add_offense(
                    keyword.range(),
                    "Missing `@return` for the value returned here.",
                );
            }
        }
    }
}

/// The `return` keyword of the first `return` in `block` (outside nested functions) that
/// returns something other than `nil`.
fn first_value_return(block: &Block) -> Option<Token> {
    block
        .statements
        .iter()
        .find_map(|statement| match &statement.kind {
            StatementKind::Return { keyword, values } => values
                .iter()
                .any(|value| !is_nil(value))
                .then_some(*keyword),
            StatementKind::Do { body }
            | StatementKind::While { body, .. }
            | StatementKind::Repeat { body, .. }
            | StatementKind::NumericFor { body, .. }
            | StatementKind::GenericFor { body, .. } => first_value_return(body),
            StatementKind::If {
                clauses, otherwise, ..
            } => clauses
                .iter()
                .map(|clause| &clause.body)
                .chain(otherwise.iter().map(|(_, body)| body))
                .find_map(first_value_return),
            _ => None,
        })
}

/// Whether `value` is the literal `nil`.
fn is_nil(value: &Expression) -> bool {
    value.unparenthesized().kind == ExpressionKind::Nil
}

inventory::submit! { Registration(|| Box::new(IncompleteDocumentation)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/IncompleteDocumentation";

    #[test]
    fn flags_undocumented_parameters() {
        expect_offenses(
            READER,
            "--- Gives things.\n-- @param actor [Player]\nfunction give(actor, item, _unused, ...) end\n",
            &[(3, 22, "Missing `@param` for `item`.")],
        );
        expect_offense(
            READER,
            "---@return number\nf = function(a)\n  return 1\nend\n",
            2,
            14,
            "Missing `@param` for `a`.",
        );
    }

    #[test]
    fn flags_a_missing_return_tag() {
        expect_offense(
            READER,
            "--- Adds.\n-- @param a [Number]\nfunction add(a)\n  if a then\n    return\n  end\n\n  \
             return a + 1\nend\n",
            8,
            3,
            "Missing `@return` for the value returned here.",
        );
    }

    #[test]
    fn ignores_nil_returns_and_nested_functions() {
        expect_no_offenses(
            READER,
            "--- Runs.\n-- @param a [Number]\nfunction run(a)\n  if !a then return nil end\n\n  \
             timer.Simple(1, function() return a end)\nend\n",
        );
    }

    #[test]
    fn accepts_complete_docs_in_every_style() {
        expect_no_offenses(
            READER,
            "--- Adds.\n-- @param a [Number]\n-- @param b=1 [Number]\n-- @return [Number]\n\
             function add(a, b) return a + b end\n\
             ---@param x number\n---@return number\nlocal function id(x) return x end\n\
             -- @tparam number y\n-- @treturn number\nfunction M:twice(y) return y * 2 end\n\
             -- @string z\nfunction say(z) print(z) end\n",
        );
    }

    #[test]
    fn needs_tags_to_check() {
        expect_no_offenses(READER, "--- Adds.\nfunction add(a, b) return a + b end\n");
        expect_no_offenses(READER, "function add(a, b) return a + b end\n");
        expect_no_offenses(
            READER,
            "--- Hook.\n-- @realm server\nfunction GM:Think(a) return a end\n",
        );
    }

    #[test]
    fn honours_typed_param_tags() {
        let src = "-- @player client desc\n-- @treturn bool\nfunction f(client) return true end\n";
        assert_eq!(inspect(READER, src).len(), 1);
        assert!(inspect_with(READER, src, "TypedParamTags: [player]").is_empty());
    }

    #[test]
    fn local_functions_are_optional() {
        let src = "-- @param a\nlocal function f(a, b) end\n\
                   -- @param a\nlocal g = function(a, b) end\n\
                   -- @param a\nfunction h(a, b) end\n";
        assert_eq!(
            inspect_with(READER, src, "RequireForLocalFunctions: true").len(),
            3
        );
        let offenses = inspect(READER, src);
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, "Missing `@param` for `b`.");
        assert_eq!((offenses[0].line, offenses[0].col), (6, 15));
    }

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn never_autocorrects() {
        let src = "-- @param a\nfunction f(a, b) end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
