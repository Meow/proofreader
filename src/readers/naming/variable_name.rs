//! `Naming/VariableName`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::case::is_lower_camel_case;
use crate::readers::naming::declarations::{DeclarationKind, declarations};

/// Flags lowerCamelCase local variables, loop variables and parameters; snake_case,
/// SCREAMING_SNAKE_CASE and ConstantStyle are allowed. `local function` names are left to
/// `Naming/MethodName`.
pub struct VariableName;

impl Reader for VariableName {
    fn name(&self) -> &'static str {
        "Naming/VariableName"
    }

    fn description(&self) -> &'static str {
        "Checks that local variables and parameters use snake_case."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for declaration in declarations(source) {
            if declaration.kind != DeclarationKind::LocalFunction
                && is_lower_camel_case(source.text_of(&declaration.token))
            {
                ctx.add_offense(
                    declaration.token.range(),
                    "Use snake_case for variable names.",
                );
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
    fn never_autocorrects() {
        let src = "local fooBar = 1\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
