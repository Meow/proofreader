//! `Performance/ExpensiveCallInHotPath`.

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::parse;
use crate::readers::performance::hot_paths::{
    DEFAULT_HOOK_FUNCTIONS, DEFAULT_HOT_PATHS, hot_calls,
};

/// The functions whose cost is too high for a function that runs every frame or tick: file
/// access, compiling code, loading materials, textures and fonts, creating panels, and
/// gathering every player or entity.
const DEFAULT_UNSAFE_FUNCTIONS: &[&str] = &[
    "player.GetAll",
    "ents.GetAll",
    "file.Append",
    "file.CreateDir",
    "file.Delete",
    "file.Exists",
    "file.Find",
    "file.IsDir",
    "file.Open",
    "file.Read",
    "file.Rename",
    "file.Size",
    "file.Time",
    "file.Write",
    "CompileString",
    "RunString",
    "RunStringEx",
    "table.HasValue",
    "surface.CreateFont",
    "surface.GetTextureID",
    "Material",
    "vgui.Create",
];

/// Flags calls of expensive functions inside hot paths: hooks and entity, weapon and panel
/// methods that run every frame or tick (`HotPaths`), whether defined as methods
/// (`function GM:Think()`, `function PANEL:Paint(w, h)`), assigned to a field, written in a
/// table constructor or registered through one of `HookFunctions` (`hook.Add('HUDPaint', ...)`).
///
/// The expensive functions are `UnsafeFunctions` plus `ExtraUnsafeFunctions` and the keys of
/// `Alternatives`, minus `SafeFunctions`; a call of a function with an alternative recommends
/// it. Files that do not parse are skipped.
pub struct ExpensiveCallInHotPath;

impl Reader for ExpensiveCallInHotPath {
    fn name(&self) -> &'static str {
        "Performance/ExpensiveCallInHotPath"
    }

    fn description(&self) -> &'static str {
        "Checks for expensive calls in functions that run every frame or tick."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("HotPaths", string_list(DEFAULT_HOT_PATHS)),
            ("HookFunctions", string_list(DEFAULT_HOOK_FUNCTIONS)),
            ("UnsafeFunctions", string_list(DEFAULT_UNSAFE_FUNCTIONS)),
            ("ExtraUnsafeFunctions", Yaml::Array(Vec::new())),
            ("SafeFunctions", Yaml::Array(Vec::new())),
            ("Alternatives", Yaml::Hash(Hash::new())),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let hot_paths = ctx.option_str_list("HotPaths", DEFAULT_HOT_PATHS);
        let hook_functions = ctx.option_str_list("HookFunctions", DEFAULT_HOOK_FUNCTIONS);
        let alternatives = ctx.option_str_map("Alternatives");
        let safe = ctx.option_str_list("SafeFunctions", &[]);
        let mut unsafe_functions = ctx.option_str_list("UnsafeFunctions", DEFAULT_UNSAFE_FUNCTIONS);
        unsafe_functions.extend(ctx.option_str_list("ExtraUnsafeFunctions", &[]));
        unsafe_functions.extend(alternatives.iter().map(|(name, _)| name.clone()));
        unsafe_functions.retain(|name| !safe.contains(name));
        for call in hot_calls(&block, source, &hot_paths, &hook_functions) {
            if !unsafe_functions.contains(&call.function) {
                continue;
            }
            let alternative = alternatives
                .iter()
                .find(|(name, _)| *name == call.function)
                .map(|(_, alternative)| alternative);
            let message = match alternative {
                Some(alternative) => format!(
                    "`{}` is expensive to call in `{}`; use `{alternative}` instead.",
                    call.function, call.hot_path
                ),
                None => format!(
                    "`{}` is expensive to call in `{}`; call it outside the hot path and keep the result.",
                    call.function, call.hot_path
                ),
            };
            ctx.add_offense(call.range, message);
        }
    }
}

/// A YAML list of the strings.
fn string_list(items: &[&str]) -> Yaml {
    Yaml::Array(
        items
            .iter()
            .map(|item| Yaml::String((*item).to_owned()))
            .collect(),
    )
}

inventory::submit! { Registration(|| Box::new(ExpensiveCallInHotPath)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/ExpensiveCallInHotPath";

    #[test]
    fn flags_expensive_calls_in_hot_paths() {
        expect_offense(
            READER,
            "function GM:HUDPaint()\n  surface.SetMaterial(Material('icon'))\nend\n",
            2,
            23,
            "`Material` is expensive to call in `HUDPaint`; call it outside the hot path and keep the result.",
        );
        expect_offense(
            READER,
            "hook.Add('Think', 'save', function()\n  file.Write('a.txt', 'x')\nend)\n",
            2,
            3,
            "`file.Write` is expensive to call in `Think`; call it outside the hot path and keep the result.",
        );
        expect_offense(
            READER,
            "local PANEL = {}\n\nfunction PANEL:Paint(w, h)\n  for _, v in ipairs(player.GetAll()) do\n    draw.text(v:Nick())\n  end\nend\n",
            4,
            22,
            "`player.GetAll` is expensive to call in `Paint`; call it outside the hot path and keep the result.",
        );
    }

    #[test]
    fn ignores_calls_outside_hot_paths() {
        expect_no_offenses(
            READER,
            "local icon = Material('icon')\n\nfunction GM:HUDPaint()\n  surface.SetMaterial(icon)\nend\n\nfunction GM:Initialize()\n  file.Write('a.txt', 'x')\nend\n",
        );
        expect_no_offenses(
            READER,
            "hook.Add('PlayerSpawn', 'x', function()\n  file.Write('a.txt', 'x')\nend)\n",
        );
    }

    #[test]
    fn recommends_alternatives() {
        expect_offense(
            READER,
            "function PANEL:Paint(w, h)\n  surface.SetMaterial(Material('icon'))\nend\n",
            2,
            23,
            "`Material` is expensive to call in `Paint`; call it outside the hot path and keep the result.",
        );
        let offenses = inspect_with(
            READER,
            "function PANEL:Paint(w, h)\n  surface.SetMaterial(Material('icon'))\n  self.font = Font.get('x')\nend\n",
            "Alternatives:\n  Material: util.get_material\n  Font.get: Font.cached",
        );
        let messages: Vec<&str> = offenses
            .iter()
            .map(|offense| offense.message.as_str())
            .collect();
        assert_eq!(
            messages,
            vec![
                "`Material` is expensive to call in `Paint`; use `util.get_material` instead.",
                "`Font.get` is expensive to call in `Paint`; use `Font.cached` instead.",
            ]
        );
    }

    #[test]
    fn honours_safe_and_extra_unsafe_functions() {
        let src = "function GM:Think()\n  local players = player.GetAll()\n  local data = util.JSONToTable(players)\nend\n";
        let offenses = inspect_with(
            READER,
            src,
            "SafeFunctions: [player.GetAll]\nExtraUnsafeFunctions: [util.JSONToTable]",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "`util.JSONToTable` is expensive to call in `Think`; call it outside the hot path and keep the result."
        );
        let offenses = inspect_with(
            READER,
            "function GM:Think()\n  Material('x')\nend\n",
            "SafeFunctions: [Material]\nAlternatives: { Material: util.get_material }",
        );
        assert!(offenses.is_empty());
    }

    #[test]
    fn honours_hot_paths_and_hook_functions() {
        let src = "function ENT:Burn()\n  file.Write('a', 'b')\nend\n\nplugin.hook('Tick', 'x', function()\n  file.Read('a')\nend)\n";
        assert!(inspect(READER, src).is_empty());
        let offenses = inspect_with(
            READER,
            src,
            "HotPaths: [Burn, Tick]\nHookFunctions: [plugin.hook]",
        );
        assert_eq!(offenses.len(), 2);
    }
}
