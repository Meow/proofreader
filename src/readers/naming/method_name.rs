//! `Naming/MethodName`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::case::is_lower_camel_case;
use crate::readers::naming::declarations::function_definitions;

/// Flags function and method definitions whose name is lowerCamelCase. PascalCase is allowed
/// because GMod hooks (`PLUGIN:PlayerSpawn`) use it.
pub struct MethodName;

impl Reader for MethodName {
    fn name(&self) -> &'static str {
        "Naming/MethodName"
    }

    fn description(&self) -> &'static str {
        "Checks that defined functions and methods use snake_case."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for definition in function_definitions(source) {
            if is_lower_camel_case(source.text_of(&definition.last)) {
                ctx.add_offense(definition.last.range(), "Use snake_case for method names.");
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(MethodName)) }

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
    fn never_autocorrects() {
        let src = "function doThing() end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
