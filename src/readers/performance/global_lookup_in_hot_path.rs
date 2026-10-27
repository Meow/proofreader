//! `Performance/GlobalLookupInHotPath`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::performance::hot_paths::configured_hot_functions;
use crate::source::Source;

/// The global libraries whose functions are commonly called every frame.
const DEFAULT_LIBRARIES: &[&str] = &[
    "math", "surface", "draw", "render", "cam", "string", "table", "util", "input", "gui",
];

/// How many lookups of one path a hot function needs by default to be reported.
const DEFAULT_MIN_USES: usize = 2;

/// Flags library members looked up through a global (`surface.SetDrawColor`, `math.sin`) at
/// least `MinUses` times in one hot path (a function under the department's `HotPaths` or
/// registered through its `HookFunctions`, with the functions nested in it). Each lookup indexes
/// the global table and then the library on every frame or tick; a file-level
/// `local SetDrawColor = surface.SetDrawColor` does it once (and `Style/PreferredLocalAlias`
/// then keeps the code using the alias). Only the libraries in `Libraries` count, and only where
/// the library name is the global rather than a local; assignments to a member are not lookups.
/// Each path is reported once per hot path, at its first lookup. Disabled by default, as caching
/// every library function is rarely worth it outside the busiest hot paths. Files that do not
/// parse are skipped.
pub struct GlobalLookupInHotPath;

impl Reader for GlobalLookupInHotPath {
    fn name(&self) -> &'static str {
        "Performance/GlobalLookupInHotPath"
    }

    fn description(&self) -> &'static str {
        "Checks for library functions looked up through their global several times in a function that runs every frame or tick."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("Libraries", yaml_strings(DEFAULT_LIBRARIES)),
            (
                "MinUses",
                Yaml::Integer(i64::try_from(DEFAULT_MIN_USES).unwrap_or(2)),
            ),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let Some(resolution) = resolve(source) else {
            return;
        };
        let libraries = ctx.option_str_list("Libraries", DEFAULT_LIBRARIES);
        let min_uses = ctx.option_usize("MinUses", DEFAULT_MIN_USES).max(1);
        let mut found = Vec::new();
        for hot in configured_hot_functions(ctx, &block) {
            let mut lookups = Lookups {
                source,
                resolution: &resolution,
                libraries: &libraries,
                assigned: HashSet::new(),
                paths: Vec::new(),
            };
            walk(&hot.function.body, &mut lookups);
            for (path, ranges) in lookups.paths {
                if ranges.len() >= min_uses
                    && let Some(first) = ranges.first()
                {
                    let message = format!(
                        "`{path}` is looked up {} times in `{}`; cache it in a file-level local.",
                        ranges.len(),
                        hot.hot_path
                    );
                    found.push((first.clone(), message));
                }
            }
        }
        found.sort_by_key(|(range, _)| range.start);
        for (range, message) in found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the lookups of library members through the global library tables in one hot path.
struct Lookups<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    libraries: &'a [String],
    /// The ranges of assignment targets, which are not lookups.
    assigned: HashSet<(usize, usize)>,
    /// Every looked up path with the ranges of its lookups, in order of the first lookup.
    paths: Vec<(String, Vec<Range<usize>>)>,
}

impl<'t> Visitor<'t> for Lookups<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        if let StatementKind::Assignment { targets, .. } = &statement.kind {
            for target in targets {
                self.assigned.insert((target.range.start, target.range.end));
            }
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Field { object, name } = &expression.kind else {
            return;
        };
        let ExpressionKind::Name(library) = &object.kind else {
            return;
        };
        let library_name = self.source.text_of(library);
        if !self.libraries.iter().any(|known| known == library_name)
            || self
                .assigned
                .contains(&(expression.range.start, expression.range.end))
            || self
                .resolution
                .reference_at(library.start)
                .is_none_or(|reference| reference.variable.is_some())
        {
            return;
        }
        let path = format!("{library_name}.{}", self.source.text_of(name));
        match self.paths.iter_mut().find(|(known, _)| *known == path) {
            Some((_, ranges)) => ranges.push(expression.range.clone()),
            None => self.paths.push((path, vec![expression.range.clone()])),
        }
    }
}

inventory::submit! { Registration(|| Box::new(GlobalLookupInHotPath)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/GlobalLookupInHotPath";

    #[test]
    fn flags_repeated_lookups_in_hot_paths() {
        expect_offenses(
            READER,
            "function PANEL:Paint(w, h)\n  surface.SetDrawColor(0, 0, 0)\n  surface.DrawRect(0, 0, w, h)\n\
             surface.SetDrawColor(255, 255, 255)\n  local s = math.sin(CurTime()) + math.sin(w)\nend\n",
            &[
                (
                    2,
                    3,
                    "`surface.SetDrawColor` is looked up 2 times in `Paint`; cache it in a file-level local.",
                ),
                (
                    5,
                    13,
                    "`math.sin` is looked up 2 times in `Paint`; cache it in a file-level local.",
                ),
            ],
        );
        expect_offense(
            READER,
            "hook.Add('HUDPaint', 'x', function()\n  for i = 1, 3 do\n    draw.SimpleText(i)\n  end\n  \
             local function shadow() draw.SimpleText('x') end\nend)\n",
            3,
            5,
            "`draw.SimpleText` is looked up 2 times in `HUDPaint`; cache it in a file-level local.",
        );
    }

    #[test]
    fn counts_each_hot_path_on_its_own() {
        expect_no_offenses(
            READER,
            "function ENT:Draw() render.SetMaterial(m) end\nfunction ENT:Think() render.SetMaterial(m) end\n\
             function ENT:Init() render.SetMaterial(m) render.SetMaterial(m) end\n",
        );
    }

    #[test]
    fn ignores_locals_other_libraries_and_assignments() {
        expect_no_offenses(
            READER,
            "local surface = surface\nfunction GM:HUDPaint()\n  surface.SetDrawColor(1) surface.SetDrawColor(2)\n\
             net.Start('a') net.Start('b')\n  math.cache = 1 math.cache = 2\n\
             local draw = {}\n  draw.Text() draw.Text()\nend\n",
        );
    }

    #[test]
    fn min_uses_and_libraries_are_configurable() {
        let src = "function GM:Think()\n  net.Start('a')\nend\n";
        assert_eq!(
            inspect_with(READER, src, "MinUses: 1\nLibraries: [net]").len(),
            1
        );
        assert!(inspect_with(READER, src, "MinUses: 1").is_empty());
    }

    #[test]
    fn hot_paths_are_shared() {
        let src = "function PANEL:Layout()\n  math.abs(1) math.abs(2)\nend\n";
        assert!(inspect(READER, src).is_empty());
        assert_eq!(
            inspect_configured(READER, src, "Performance:\n  HotPaths: [Layout]\n").len(),
            1
        );
    }

    #[test]
    fn is_disabled_by_default() {
        let reader = crate::reader::reader_named(READER).expect("registered");
        assert!(!reader.default_enabled());
    }
}
