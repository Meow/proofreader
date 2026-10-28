//! `Naming/MethodName`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::naming::case::{Case, NamingStyle, classify};
use crate::readers::naming::declarations::{FunctionDefinition, function_definitions};
use crate::source::Source;

/// Checks the last name segment of function and method definitions (including
/// `local function`) against `EnforcedStyle`.
///
/// With `snake_case` (the default) lowerCamelCase names are flagged, and PascalCase names only
/// where GMod and Lumen need them: methods (`PLUGIN:PlayerSpawn`, hooks are called by name),
/// functions of tables named in capitals (`TOOL.BuildCPanel`), functions added to or replacing
/// those of a lowercase global library (`function hook.Call()`, `function table.Merge()`), global
/// functions in the style of the GMod API (`function LerpColor()`) and, in Lumen templates, local
/// functions used as component tags (`<Icon />`); a PascalCase `local function` helper or
/// `Config.GetThing` is flagged. With `camelCase` snake_case names are
/// flagged instead, and with `PascalCase` (vanilla GMod and Clockwork) every name starting with
/// a lowercase letter. SCREAMING_SNAKE_CASE names suit every style, and Lua's metamethod names
/// (`__index`, `__tostring`, ...) are never flagged. `LocalFunctionStyle` gives `local function`
/// names a style of their own (the Helix style names them like local variables); with `same` (the
/// default) they follow `EnforcedStyle`.
pub struct MethodName;

impl Reader for MethodName {
    fn name(&self) -> &'static str {
        "Naming/MethodName"
    }

    fn description(&self) -> &'static str {
        "Checks that defined functions and methods follow the configured naming style."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("EnforcedStyle", Yaml::String("snake_case".to_owned())),
            ("LocalFunctionStyle", Yaml::String("same".to_owned())),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let enforced = ctx.option_str("EnforcedStyle", "snake_case");
        let style = NamingStyle::from_option(enforced);
        let (local_style, local_noun) = match ctx.option_str("LocalFunctionStyle", "same") {
            "same" => (style, "method"),
            other => (NamingStyle::from_option(other), "local function"),
        };
        let source = ctx.source;
        let mut resolution = None;
        for definition in function_definitions(source) {
            let name = source.text_of(&definition.last);
            let (style, noun) = if definition.local {
                (local_style, local_noun)
            } else {
                (style, "method")
            };
            if METAMETHODS.contains(&name) {
                continue;
            }
            let pascal_not_allowed = style == NamingStyle::Snake
                && classify(name) == Case::Pascal
                && !pascal_allowed(source, &definition, &mut resolution);
            if style.rejects(name) || pascal_not_allowed {
                ctx.add_offense(
                    definition.last.range(),
                    format!("Use {} for {noun} names.", style.label()),
                );
            }
        }
    }
}

/// Whether the PascalCase name of `definition` is one GMod or Lumen needs: a method, a function
/// of a table named in capitals or of a lowercase global library, a global function or a Lumen
/// component. `resolution` caches the scope resolution of `source`, computed when first needed.
fn pascal_allowed<'a>(
    source: &'a Source,
    definition: &FunctionDefinition,
    resolution: &mut Option<Option<Resolution<'a>>>,
) -> bool {
    if definition.name.contains(':') {
        return true;
    }
    if !definition.local && !definition.name.contains('.') {
        return true;
    }
    if let Some((owner, _)) = definition.name.split_once('.') {
        let capitals = owner.chars().any(|c| c.is_ascii_uppercase())
            && owner
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if capitals {
            return true;
        }
        if !owner.starts_with(|c: char| c.is_ascii_lowercase()) {
            return false;
        }
        let index = source
            .code_token_indexes()
            .partition_point(|&position| source.tokens[position].start <= definition.keyword.start);
        let Some(owner_token) = source.code_token(index) else {
            return false;
        };
        let resolved = resolution.get_or_insert_with(|| resolve(source));
        return resolved.as_ref().is_some_and(|resolved| {
            resolved
                .reference_at(owner_token.start)
                .is_some_and(|reference| reference.variable.is_none())
        });
    }
    let name = source.text_of(&definition.last);
    source
        .component_references()
        .iter()
        .any(|(component, _)| *component == name)
}

/// The metamethod names Lua and LuaJIT look up, which keep their spelling in every style.
const METAMETHODS: &[&str] = &[
    "__add",
    "__band",
    "__bnot",
    "__bor",
    "__bxor",
    "__call",
    "__close",
    "__concat",
    "__div",
    "__eq",
    "__gc",
    "__idiv",
    "__index",
    "__ipairs",
    "__le",
    "__len",
    "__lt",
    "__metatable",
    "__mod",
    "__mode",
    "__mul",
    "__name",
    "__newindex",
    "__pairs",
    "__pow",
    "__shl",
    "__shr",
    "__sub",
    "__tostring",
    "__unm",
];

inventory::submit! { Registration(|| Box::new(MethodName)) }

#[cfg(test)]
mod pascal_case_tests {
    use crate::testing::*;

    const READER: &str = "Naming/MethodName";
    const MESSAGE: &str = "Use snake_case for method names.";

    #[test]
    fn pascal_case_only_where_gmod_or_lumen_need_it() {
        expect_no_offenses(
            READER,
            "function PLUGIN:PlayerSpawn() end\nfunction TOOL.BuildCPanel() end\n\
             function LerpColor() end\nfunction Config.get() end\nfunction hook.Call() end\n\
             function table.Merge() end\n",
        );
        expect_offenses(
            READER,
            "local function CheckConditions() end\nfunction Config.GetColor() end\n\
             local helpers = {}\nfunction helpers.DoThing() end\n",
            &[(1, 16, MESSAGE), (2, 17, MESSAGE), (4, 18, MESSAGE)],
        );
    }

    #[test]
    fn constant_functions_are_local_functions() {
        expect_offense(READER, "const function doThing() end\n", 1, 16, MESSAGE);
        assert!(
            inspect_with(
                READER,
                "const function doThing() end\n",
                "LocalFunctionStyle: camelCase"
            )
            .is_empty()
        );
    }

    #[test]
    fn lumen_components_may_be_pascal_case() {
        expect_no_offenses_lumen(
            READER,
            "local function Icon(props)\n  return <text>{props.name}</text>\nend\n\nreturn <Icon name='x' />\n",
        );
        expect_offense_lumen(
            READER,
            "local function Helper() end\n\nreturn <view />\n",
            1,
            16,
            MESSAGE,
        );
    }

    #[test]
    fn other_styles_keep_their_rules() {
        assert!(
            inspect_with(
                READER,
                "local function CheckConditions() end\n",
                "EnforcedStyle: PascalCase"
            )
            .is_empty()
        );
    }
}

#[cfg(test)]
mod local_function_style_tests {
    use crate::testing::*;

    const READER: &str = "Naming/MethodName";

    #[test]
    fn local_functions_can_have_their_own_style() {
        let options = "EnforcedStyle: PascalCase\nLocalFunctionStyle: camelCase";
        let offenses = inspect_with(
            READER,
            "local function doThing() end\nlocal function do_thing() end\n\
             function PLUGIN:Think() end\nfunction PLUGIN:think() end\n",
            options,
        );
        let found: Vec<(u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (2, "Use camelCase for local function names."),
                (4, "Use PascalCase for method names.")
            ]
        );
        let same = inspect_with(
            READER,
            "local function doThing() end\n",
            "EnforcedStyle: PascalCase",
        );
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].message, "Use PascalCase for method names.");
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Naming/MethodName";
    const MESSAGE: &str = "Use snake_case for method names.";

    #[test]
    fn flags_camel_case_definitions() {
        expect_offense(READER, "function doThing() end\n", 1, 10, MESSAGE);
        expect_offense(READER, "function obj.lib:doThing() end\n", 1, 18, MESSAGE);
        expect_offense(READER, "local function doThing() end\n", 1, 16, MESSAGE);
    }

    #[test]
    fn accepts_snake_and_pascal_case() {
        expect_no_offenses(
            READER,
            "function do_thing() end\nfunction PLUGIN:SetupMove(a) end\nfunction Foo.bar_baz() end\n",
        );
    }

    #[test]
    fn checks_only_the_last_segment() {
        expect_no_offenses(READER, "function someLib.do_thing() end\n");
    }

    #[test]
    fn ignores_calls_and_anonymous_functions() {
        expect_no_offenses(
            READER,
            "obj:doThing()\nlocal f = function(x) end\ndoThing()\n",
        );
    }

    #[test]
    fn camel_case_style_flags_snake_case() {
        let src = "function do_thing() end\nfunction obj:doThing() end\n\
                   function GM:PlayerSpawn() end\nlocal function helper_fn() end\n";
        let found: Vec<(u32, u32, String)> = inspect_with(READER, src, "EnforcedStyle: camelCase")
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect();
        let message = "Use camelCase for method names.".to_owned();
        assert_eq!(found, vec![(1, 10, message.clone()), (4, 16, message)]);
    }

    #[test]
    fn pascal_case_style_flags_lowercase_names() {
        let src = "function GM:PlayerSpawn() end\nfunction obj:think() end\n\
                   function lib.doThing() end\nlocal function add_hook() end\n\
                   function lib.INIT() end\n";
        let found: Vec<(u32, u32, String)> = inspect_with(READER, src, "EnforcedStyle: PascalCase")
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect();
        let message = "Use PascalCase for method names.".to_owned();
        assert_eq!(
            found,
            vec![
                (2, 14, message.clone()),
                (3, 14, message.clone()),
                (4, 16, message)
            ]
        );
    }

    #[test]
    fn metamethods_keep_their_names() {
        expect_no_offenses(READER, "function META:__tostring() end\n");
        assert!(
            inspect_with(
                READER,
                "function META:__index(k) end\nfunction mt.__call() end\n",
                "EnforcedStyle: PascalCase"
            )
            .is_empty()
        );
    }

    #[test]
    fn never_autocorrects() {
        let src = "function doThing() end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
