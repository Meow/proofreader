//! `Lint/UndefinedDocParam`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::parse;
use crate::readers::style::doc_comments::{LDOC_TYPED_PARAM_TAGS, functions};

/// Flags parameter tags in a function's doc comment that name no parameter of the function.
///
/// Parameter tags are `@param name` (Flux `-- @param name=default [Type]`, EmmyLua
/// `---@param name? type`), LDoc's `@tparam type name` and the `TypedParamTags` aliases of
/// `@tparam` (`@string name`). A method has an implicit `self`, and `...` matches a vararg
/// function. The doc comment is the comment-only `--` lines directly above the line of the
/// `function` keyword, when it is the first function on its line. Files that do not parse are
/// skipped.
pub struct UndefinedDocParam;

impl Reader for UndefinedDocParam {
    fn name(&self) -> &'static str {
        "Lint/UndefinedDocParam"
    }

    fn description(&self) -> &'static str {
        "Checks for `@param` tags that name no parameter of the documented function."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![(
            "TypedParamTags",
            Yaml::Array(
                LDOC_TYPED_PARAM_TAGS
                    .iter()
                    .map(|tag| Yaml::String((*tag).to_owned()))
                    .collect(),
            ),
        )]
    }

    fn investigate(&self, ctx: &mut Context) {
        let typed = ctx.option_str_list("TypedParamTags", &LDOC_TYPED_PARAM_TAGS);
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        for function in functions(source, &block) {
            let Some(doc) = &function.doc else {
                continue;
            };
            for param in doc.params(&typed) {
                if function.has_parameter(source, param.name) {
                    continue;
                }
                ctx.add_offense(
                    param.range.clone(),
                    format!(
                        "`@param {}` does not match a parameter of {}.",
                        param.name,
                        function.describe()
                    ),
                );
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(UndefinedDocParam)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/UndefinedDocParam";

    #[test]
    fn flags_params_the_function_does_not_have() {
        expect_offense(
            READER,
            "--- Does things.\n-- @param actor [Player]\n-- @param amount=1 [Number]\n\
             function give(actor, count) end\n",
            3,
            11,
            "`@param amount` does not match a parameter of `give`.",
        );
        expect_offenses(
            READER,
            "---@param a string\n---@param b? number\nlocal function f(a) end\n\
             -- @param x [X]\nlocal g = function() end\n",
            &[
                (2, 11, "`@param b` does not match a parameter of `f`."),
                (4, 11, "`@param x` does not match a parameter of `g`."),
            ],
        );
        expect_offense(
            READER,
            "-- @param ply [Player]\nhook.Add('PlayerSpawn', 'x', function(client) end)\n",
            1,
            11,
            "`@param ply` does not match a parameter of this function.",
        );
    }

    #[test]
    fn understands_self_varargs_and_ldoc_tags() {
        expect_no_offenses(
            READER,
            "---@param self Player\n---@param ... any\nfunction PLAYER:notify(...) end\n",
        );
        expect_offenses(
            READER,
            "---@param self table\n---@param ... any\nfunction M.f() end\n",
            &[
                (1, 11, "`@param self` does not match a parameter of `M.f`."),
                (2, 11, "`@param ...` does not match a parameter of `M.f`."),
            ],
        );
        expect_offenses(
            READER,
            "-- @tparam string a desc\n-- @string b desc\n-- @player c desc\nfunction f(x) end\n",
            &[
                (1, 19, "`@param a` does not match a parameter of `f`."),
                (2, 12, "`@param b` does not match a parameter of `f`."),
            ],
        );
        assert_eq!(
            inspect_with(
                READER,
                "-- @player c desc\nfunction f(x) end\n",
                "TypedParamTags: [player]"
            )
            .len(),
            1
        );
    }

    #[test]
    fn accepts_matching_and_variant_docs() {
        expect_no_offenses(
            READER,
            "--- Makes a color.\n-- @variant Color(r, g)\n--   @param r [String name]\n\
             --   @param g=255 [Number alpha]\n-- @return [Color]\nfunction Color(r, g, b) end\n",
        );
    }

    #[test]
    fn ignores_comments_that_are_not_doc_comments() {
        expect_no_offenses(READER, "-- @param a\n\nfunction f() end\n");
        expect_no_offenses(READER, "-- @param a\nlocal x = 1\n");
        expect_no_offenses(READER, "-- @param a\nfunction f(\n");
    }

    #[test]
    fn the_first_function_on_the_line_owns_the_comment() {
        expect_offense(
            READER,
            "-- @param a\nf(function() end, function(a) end)\n",
            1,
            11,
            "`@param a` does not match a parameter of this function.",
        );
    }

    #[test]
    fn checks_nested_functions() {
        expect_offense(
            READER,
            "function f()\n  -- @param y\n  local g = function(x) end\nend\n",
            2,
            13,
            "`@param y` does not match a parameter of `g`.",
        );
    }

    #[test]
    fn checks_short_functions() {
        expect_offense(
            READER,
            "--- Sums.\n-- @param a [Number]\n-- @param c [Number]\nlocal sum = |a, b| -> a + b\n",
            3,
            11,
            "`@param c` does not match a parameter of `sum`.",
        );
        expect_no_offenses(
            READER,
            "--- Counts.\n-- @param ... [Any]\nlocal count = |...| -> select('#', ...)\n",
        );
    }

    #[test]
    fn never_autocorrects() {
        let src = "-- @param a\nfunction f() end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
