//! `Lint/RedefinedLocal`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::source::Source;

/// Calls that register a panel table, with the index of the table argument. A file may reuse
/// `local PANEL = {}` once the previous table has been handed to one of them.
const PANEL_REGISTRATIONS: [(&str, usize); 3] = [
    ("vgui.Register", 1),
    ("vgui.RegisterTable", 0),
    ("derma.DefineControl", 2),
];

/// Flags a local variable, `local function`, parameter or loop variable whose name is already
/// a local visible where it is declared: an earlier declaration in the same scope or one in an
/// enclosing scope, across function boundaries.
///
/// Names starting with `_`, `self` and the implicit `self` of methods are exempt. `local x = x`
/// shadows the outer `x` (the right-hand side still reads it) and is reported. Redeclaring a
/// panel table after passing the previous one to `vgui.Register`, `vgui.RegisterTable` or
/// `derma.DefineControl` (`local PANEL = {}` once per panel) is accepted. With
/// `AllowParameters`, parameters may shadow outer locals, but not each other. Files that do not
/// parse are skipped.
pub struct RedefinedLocal;

impl Reader for RedefinedLocal {
    fn name(&self) -> &'static str {
        "Lint/RedefinedLocal"
    }

    fn description(&self) -> &'static str {
        "Checks for locals that shadow a local of the same name."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("AllowParameters", Yaml::Boolean(false))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let allow_parameters = ctx.option_bool("AllowParameters", false);
        let source = ctx.source;
        let Some(resolution) = resolve(source) else {
            return;
        };
        let mut registrations: Option<Vec<(usize, usize)>> = None;
        for variable in &resolution.variables {
            let Some(shadowed) = variable.shadows.map(|id| &resolution.variables[id]) else {
                continue;
            };
            if variable.implicit
                || shadowed.implicit
                || variable.name.starts_with('_')
                || variable.name == "self"
            {
                continue;
            }
            if allow_parameters
                && variable.kind == DeclarationKind::Parameter
                && !(shadowed.kind == DeclarationKind::Parameter
                    && shadowed.function == variable.function)
            {
                continue;
            }
            let registered = registrations
                .get_or_insert_with(|| panel_registrations(source, &resolution))
                .iter()
                .any(|&(call, shadowed_id)| {
                    Some(shadowed_id) == variable.shadows && call < variable.scope.end
                });
            if registered {
                continue;
            }
            ctx.add_offense(
                variable.token.range(),
                format!(
                    "`{}` shadows the local declared on line {}.",
                    variable.name, shadowed.token.line
                ),
            );
        }
    }
}

/// The panel registration calls of `source`, as the byte offset of the call and the variable
/// passed as the panel table.
fn panel_registrations(source: &Source, resolution: &Resolution) -> Vec<(usize, usize)> {
    let Ok(block) = parse(source) else {
        return Vec::new();
    };
    let mut finder = Registrations {
        source,
        resolution,
        found: Vec::new(),
    };
    walk(&block, &mut finder);
    finder.found
}

/// Collects panel registration calls whose table argument is a local.
struct Registrations<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    found: Vec<(usize, usize)>,
}

impl<'t> Visitor<'t> for Registrations<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let Some((name, arguments)) = expression.as_call(self.source) else {
            return;
        };
        let Some(&(_, index)) = PANEL_REGISTRATIONS
            .iter()
            .find(|(registration, _)| *registration == name)
        else {
            return;
        };
        if let Some(ExpressionKind::Name(token)) = arguments.values.get(index).map(|arg| &arg.kind)
            && let Some(variable) = self
                .resolution
                .reference_at(token.start)
                .and_then(|reference| reference.variable)
        {
            self.found.push((expression.range.start, variable));
        }
    }
}

inventory::submit! { Registration(|| Box::new(RedefinedLocal)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/RedefinedLocal";

    fn message(name: &str, line: u32) -> String {
        format!("`{name}` shadows the local declared on line {line}.")
    }

    #[test]
    fn flags_redeclarations_in_the_same_scope() {
        expect_offense(
            READER,
            "local ok = f()\nlocal ok = g()\nprint(ok)\n",
            2,
            7,
            &message("ok", 1),
        );
        expect_offense(READER, "local a, a = 1, 2\n", 1, 10, &message("a", 1));
        expect_offense(READER, "local x = 1\nlocal x = x\n", 2, 7, &message("x", 1));
    }

    #[test]
    fn flags_every_declaration_kind_in_nested_scopes() {
        expect_offenses(
            READER,
            "local v = 1\nlocal function f(v)\n  for v = 1, 2 do\n    local function v() end\n  \
             end\n  for k, v in pairs(t) do end\nend\n",
            &[
                (2, 18, &message("v", 1)),
                (3, 7, &message("v", 2)),
                (4, 20, &message("v", 3)),
                (6, 10, &message("v", 2)),
            ],
        );
        expect_offense(
            READER,
            "local ply = 1\nhook.Add('A', 'b', function(ply) end)\n",
            2,
            29,
            &message("ply", 1),
        );
        expect_offense(
            READER,
            "local function f() end\nlocal function f() end\n",
            2,
            16,
            &message("f", 1),
        );
    }

    #[test]
    fn accepts_separate_scopes() {
        expect_no_offenses(
            READER,
            "do local a = 1 end\ndo local a = 2 end\nif x then local b = 1 else local b = 2 end\n\
             function f(a) end\nfunction g(a) end\nlocal f = function(f) end\n",
        );
        expect_no_offenses(READER, "x = 1\nlocal x = x\n");
    }

    #[test]
    fn exempts_underscores_and_self() {
        expect_no_offenses(
            READER,
            "local _ = 1\nlocal _ = 2\nfor _, v in pairs(t) do for _, w in pairs(v) do end end\n\
             local self = 1\nfunction a:b() local self = 2 end\n",
        );
    }

    #[test]
    fn accepts_panel_tables_reused_after_registration() {
        expect_no_offenses(
            READER,
            "local PANEL = {}\nvgui.Register('A', PANEL, 'DPanel')\n\nlocal PANEL = {}\n\
             derma.DefineControl('B', 'b', PANEL, 'DPanel')\nlocal PANEL = {}\n\
             vgui.RegisterTable(PANEL)\nlocal PANEL = vgui.RegisterTable(PANEL)\n",
        );
        expect_offense(
            READER,
            "local PANEL = {}\nlocal PANEL = {}\nvgui.Register('A', PANEL, 'DPanel')\n",
            2,
            7,
            &message("PANEL", 1),
        );
    }

    #[test]
    fn parameters_may_be_allowed() {
        let src = "local ply = 1\nhook.Add('A', 'b', function(ply) local ply = 2 end)\n\
                   function f(a, a) end\n";
        let offenses = inspect_with(READER, src, "AllowParameters: true");
        let found: Vec<(u32, u32)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col))
            .collect();
        assert_eq!(found, vec![(2, 40), (3, 15)]);
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "local a = 1\nlocal a = 2\nif x then\n");
    }

    #[test]
    fn checks_lumen_templates() {
        expect_offense_lumen(
            READER,
            "local Row = 1\nreturn function(Row)\n  return <Row />\nend\n",
            2,
            17,
            &message("Row", 1),
        );
    }

    #[test]
    fn never_autocorrects() {
        let src = "local a = 1\nlocal a = 2\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
