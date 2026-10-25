//! `Naming/VariableName`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::case::NamingStyle;
use crate::readers::naming::declarations::{DeclarationKind, declarations};

/// Checks the names of local variables, loop variables and parameters against `EnforcedStyle`.
///
/// With `snake_case` (the default) lowerCamelCase names are flagged; snake_case,
/// SCREAMING_SNAKE_CASE and ConstantStyle are allowed. With `camelCase` snake_case names are
/// flagged instead, and with `PascalCase` every name starting with a lowercase letter. Single
/// lowercase words (`ply`) suit `snake_case` and `camelCase`, and SCREAMING_SNAKE_CASE constants
/// suit every style. `local function` names are left to `Naming/MethodName`.
pub struct VariableName;

impl Reader for VariableName {
    fn name(&self) -> &'static str {
        "Naming/VariableName"
    }

    fn description(&self) -> &'static str {
        "Checks that local variables and parameters follow the configured naming style."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("snake_case".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let style = NamingStyle::from_option(ctx.option_str("EnforcedStyle", "snake_case"));
        let message = format!("Use {} for variable names.", style.label());
        let source = ctx.source;
        for declaration in declarations(source) {
            if declaration.kind != DeclarationKind::LocalFunction
                && style.rejects(source.text_of(&declaration.token))
            {
                ctx.add_offense(declaration.token.range(), message.clone());
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(VariableName)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Naming/VariableName";
    const MESSAGE: &str = "Use snake_case for variable names.";

    #[test]
    fn flags_camel_case_locals() {
        expect_offense(READER, "local fooBar = 1\n", 1, 7, MESSAGE);
        expect_offense(READER, "local a, fooBar = 1, 2\n", 1, 10, MESSAGE);
        expect_offense(READER, "local _privateThing\n", 1, 7, MESSAGE);
    }

    #[test]
    fn leaves_local_function_names_to_method_name() {
        expect_no_offenses(READER, "local function doThing() end\n");
        expect_offense(
            READER,
            "local function do_thing(someArg) end\n",
            1,
            25,
            MESSAGE,
        );
    }

    #[test]
    fn flags_camel_case_parameters_and_loop_variables() {
        expect_offense(READER, "function f(a, someArg) end\n", 1, 15, MESSAGE);
        expect_offense(READER, "x = function(someArg) end\n", 1, 14, MESSAGE);
        expect_offense(READER, "for plyID, v in pairs(t) do end\n", 1, 5, MESSAGE);
        expect_offense(READER, "for camelIndex = 1, 2 do end\n", 1, 5, MESSAGE);
    }

    #[test]
    fn accepts_other_styles() {
        expect_no_offenses(
            READER,
            "local snake_case, SCREAMING_SNAKE, ConstantStyle, x, trailing_ = 1\n\
             local function PascalHook(actor, _) end\nfor _, v in ipairs(t) do end\n",
        );
    }

    #[test]
    fn ignores_fields_globals_and_strings() {
        expect_no_offenses(READER, "self.fooBar = 1\nfooBar = 2\nlocal s = 'fooBar'\n");
        expect_no_offenses(READER, "function obj:doThing(x) end\n-- local fooBar\n");
    }

    #[test]
    fn camel_case_style_flags_snake_case() {
        let src = "local my_var, myVar, ply, MAX_PLAYERS, ModelList = 1\n\
                   function f(some_arg, otherArg) end\nfor _, v_x in ipairs(t) do end\n\
                   local function do_thing(x) end\n";
        let found: Vec<(u32, u32, String)> = inspect_with(READER, src, "EnforcedStyle: camelCase")
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect();
        let message = "Use camelCase for variable names.".to_owned();
        assert_eq!(
            found,
            vec![
                (1, 7, message.clone()),
                (2, 12, message.clone()),
                (3, 8, message)
            ]
        );
    }

    #[test]
    fn pascal_case_style_flags_lowercase_names() {
        let found: Vec<(u32, u32)> = inspect_with(
            READER,
            "local ply, ModelList, MAX, someVar, some_var = 1\n",
            "EnforcedStyle: PascalCase",
        )
        .into_iter()
        .map(|offense| (offense.line, offense.col))
        .collect();
        assert_eq!(found, vec![(1, 7), (1, 28), (1, 37)]);
        assert_eq!(
            inspect_with(READER, "local x\n", "EnforcedStyle: PascalCase")[0].message,
            "Use PascalCase for variable names."
        );
    }

    #[test]
    fn never_autocorrects() {
        let src = "local fooBar = 1\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
