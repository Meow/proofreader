//! `Performance/ConstantObjectInHotPath`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::parse;
use crate::readers::performance::hot_paths::{configured_hot_calls, is_constant};

/// The constructors whose result never changes when every argument is a constant.
const DEFAULT_FUNCTIONS: &[&str] = &["Color", "Vector", "Angle"];

/// Longest call text quoted in a message; longer calls are shown as `Name(...)`.
const MAX_QUOTED_CALL: usize = 60;

/// Flags constructors called with only constant arguments (`Color(255, 0, 0)`, `Vector(0, 0, 1)`)
/// inside hot paths (the department's `HotPaths` and `HookFunctions`; see
/// `Performance/ExpensiveCallInHotPath`), which build the same object every frame or tick when a
/// local outside the hot path could hold it. `Functions` lists the constructors; a call without
/// arguments is not flagged. Files that do not parse are skipped.
pub struct ConstantObjectInHotPath;

impl Reader for ConstantObjectInHotPath {
    fn name(&self) -> &'static str {
        "Performance/ConstantObjectInHotPath"
    }

    fn description(&self) -> &'static str {
        "Checks for objects built from constants in functions that run every frame or tick."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("Functions", yaml_strings(DEFAULT_FUNCTIONS))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let functions = ctx.option_str_list("Functions", DEFAULT_FUNCTIONS);
        for call in configured_hot_calls(ctx, &block) {
            if !functions.contains(&call.function)
                || call.arguments.values.is_empty()
                || !call.arguments.values.iter().all(is_constant)
            {
                continue;
            }
            let text = &source.text[call.range.clone()];
            let shown = if text.len() <= MAX_QUOTED_CALL && !text.contains('\n') {
                text.to_owned()
            } else {
                format!("{}(...)", call.function)
            };
            ctx.add_offense(
                call.range,
                format!(
                    "`{shown}` is built from constants every time `{}` runs; keep it in a local outside the hot path.",
                    call.hot_path
                ),
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(ConstantObjectInHotPath)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/ConstantObjectInHotPath";

    #[test]
    fn flags_constant_objects_in_hot_paths() {
        expect_offense(
            READER,
            "function PANEL:Paint(w, h)\n  draw.RoundedBox(4, 0, 0, w, h, Color(0, 0, 0, 200))\nend\n",
            2,
            34,
            "`Color(0, 0, 0, 200)` is built from constants every time `Paint` runs; keep it in a local outside the hot path.",
        );
        expect_offense(
            READER,
            "hook.Add('Think', 'x', function()\n  local offset = Vector(0, 0, -16 * 2)\nend)\n",
            2,
            18,
            "`Vector(0, 0, -16 * 2)` is built from constants every time `Think` runs; keep it in a local outside the hot path.",
        );
    }

    #[test]
    fn short_functions_can_be_hot_paths() {
        expect_offense(
            READER,
            "hook.Add('HUDPaint', 'x', || -> draw.Text(Color(0, 0, 1 << 4)))\n",
            1,
            43,
            "`Color(0, 0, 1 << 4)` is built from constants every time `HUDPaint` runs; keep it in a local outside the hot path.",
        );
        expect_offense(
            READER,
            "PANEL.Paint = |panel, w, h| -> do\n  draw.Box(Color(1, 2, 3))\nend\n",
            2,
            12,
            "`Color(1, 2, 3)` is built from constants every time `Paint` runs; keep it in a local outside the hot path.",
        );
    }

    #[test]
    fn ignores_variable_arguments_and_other_contexts() {
        expect_no_offenses(
            READER,
            "function PANEL:Paint(w, h)\n  draw.RoundedBox(4, 0, 0, w, h, Color(0, 0, 0, alpha))\n  local v = Vector()\nend\n",
        );
        expect_no_offenses(
            READER,
            "local black = Color(0, 0, 0)\n\nfunction PANEL:Init()\n  self.color = Color(0, 0, 0)\nend\n",
        );
    }

    #[test]
    fn shortens_long_calls_and_honours_functions() {
        let src =
            "function GM:HUDPaint()\n  local c = Color(\n    255,\n    255,\n    255\n  )\nend\n";
        let offenses = inspect(READER, src);
        assert_eq!(offenses.len(), 1);
        assert!(offenses[0].message.starts_with("`Color(...)` is built"));
        let offenses = inspect_with(
            READER,
            "function GM:HUDPaint()\n  local c = Color(1, 2, 3)\n  local m = Matrix({})\n  local r = Rect(1, 2)\nend\n",
            "Functions: [Rect]",
        );
        assert_eq!(offenses.len(), 1);
        assert!(offenses[0].message.starts_with("`Rect(1, 2)`"));
    }
}
