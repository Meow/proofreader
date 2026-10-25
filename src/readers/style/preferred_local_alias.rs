//! `Style/PreferredLocalAlias`.

use std::ops::Range;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;

/// Flags uses of a dotted path, such as `table.insert`, where a local alias of it is in scope
/// (`local insert = table.insert`); the fix replaces the path with the alias.
///
/// An alias is a local declared at the top level of the file, initialised with a path of two or
/// more names starting with a global, and never assigned afterwards: the usual way of caching a
/// library function. Locals copied from fields of other locals, parameters or `self` (`local
/// name = data.name`) are snapshots of data that may change, not aliases. A use is flagged when
/// it comes after the alias statement, the alias name is not shadowed there, the first name of
/// the path is still the global, the chain starts with that name (not `x.table.insert`) and the
/// use is not itself assigned to. Paths that are assigned anywhere in
/// the file (`table.insert = ...`, `function table.insert()`, `table = ...`) are never flagged.
/// Files that do not parse are skipped.
pub struct PreferredLocalAlias;

impl Reader for PreferredLocalAlias {
    fn name(&self) -> &'static str {
        "Style/PreferredLocalAlias"
    }

    fn description(&self) -> &'static str {
        "Checks for paths used where a local alias of them is in scope."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let (Ok(block), Some(resolution)) = (parse(source), resolve(source)) else {
            return;
        };
        let mut collector = Collector {
            source,
            depth: 0,
            aliases: Vec::new(),
            uses: Vec::new(),
            assigned: Vec::new(),
            targets: Vec::new(),
        };
        walk(&block, &mut collector);
        let aliases: Vec<Alias> = collector
            .aliases
            .iter()
            .filter(|alias| {
                !collector
                    .assigned
                    .iter()
                    .any(|assigned| covers(assigned, &alias.path))
            })
            .filter_map(|alias| alias.resolve(&resolution))
            .collect();
        for path_use in &collector.uses {
            if collector.targets.contains(&path_use.range) {
                continue;
            }
            let Some(alias) = aliases
                .iter()
                .filter(|alias| alias.applies_to(path_use, &resolution))
                .max_by_key(|alias| alias.statement_end)
            else {
                continue;
            };
            let message = format!(
                "Use the local alias `{}` instead of `{}`.",
                alias.name,
                alias.path.join(".")
            );
            let compact = source.text.get(path_use.range.clone()) == Some(&alias.path.join("."));
            if compact {
                ctx.add_offense_with_fix(
                    path_use.range.clone(),
                    message,
                    vec![Edit::replace(path_use.range.clone(), alias.name.clone())],
                );
            } else {
                ctx.add_offense(path_use.range.clone(), message);
            }
        }
    }
}

/// Whether assigning the path `assigned` changes the value of `path`: it is `path` itself or
/// one of its prefixes.
fn covers(assigned: &[String], path: &[String]) -> bool {
    assigned.len() <= path.len() && path[..assigned.len()] == *assigned
}

/// A `local name = a.b.c` statement found in the tree.
struct AliasCandidate {
    /// The alias name.
    name: String,
    /// Byte offset of the alias name token.
    name_start: usize,
    /// The aliased path (`["a", "b", "c"]`).
    path: Vec<String>,
    /// Byte offset of the first name of the path in the alias statement.
    base_start: usize,
    /// End of the alias statement.
    statement_end: usize,
}

/// An alias whose variable and base binding are known.
struct Alias {
    name: String,
    path: Vec<String>,
    /// The alias variable.
    variable: usize,
    statement_end: usize,
    scope_end: usize,
}

impl AliasCandidate {
    /// Binds the candidate to its variable; `None` when the variable is assigned later.
    fn resolve(&self, resolution: &Resolution) -> Option<Alias> {
        let (id, variable) = resolution
            .variables
            .iter()
            .enumerate()
            .find(|(_, variable)| variable.token.start == self.name_start)?;
        if !variable.writes.is_empty()
            || resolution.reference_at(self.base_start)?.variable.is_some()
        {
            return None;
        }
        Some(Alias {
            name: self.name.clone(),
            path: self.path.clone(),
            variable: id,
            statement_end: self.statement_end,
            scope_end: variable.scope.end,
        })
    }
}

impl Alias {
    /// Whether `path_use` may be replaced by this alias.
    fn applies_to(&self, path_use: &PathUse, resolution: &Resolution) -> bool {
        let offset = path_use.range.start;
        path_use.path == self.path
            && offset >= self.statement_end
            && offset < self.scope_end
            && resolution.visible_at(&self.name, offset) == Some(self.variable)
            && resolution
                .reference_at(offset)
                .is_some_and(|reference| reference.variable.is_none())
    }
}

/// A field chain starting with a name (`a.b.c`) used in the code.
struct PathUse {
    path: Vec<String>,
    range: Range<usize>,
}

/// Collects alias statements, path uses and assigned paths.
struct Collector<'a> {
    source: &'a Source,
    /// How many blocks enclose the current statement (1 at the top level of the file).
    depth: usize,
    aliases: Vec<AliasCandidate>,
    uses: Vec<PathUse>,
    /// Every assigned name or field path (assignment targets and function statement names).
    assigned: Vec<Vec<String>>,
    /// Byte ranges of the field expressions that are assignment targets.
    targets: Vec<Range<usize>>,
}

/// The names of a name or field chain (`["a", "b", "c"]` for `a.b.c`) and the first name's
/// byte offset.
fn path_of(source: &Source, expression: &Expression) -> Option<(Vec<String>, usize)> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some((vec![source.text_of(token).to_owned()], token.start)),
        ExpressionKind::Field { object, name } => {
            let (mut path, start) = path_of(source, object)?;
            path.push(source.text_of(name).to_owned());
            Some((path, start))
        }
        _ => None,
    }
}

impl<'t> Visitor<'t> for Collector<'_> {
    fn enter_block(&mut self, _block: &'t Block) {
        self.depth += 1;
    }

    fn leave_block(&mut self, _block: &'t Block) {
        self.depth -= 1;
    }

    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Local { names, values, .. } if self.depth == 1 => {
                for (name, value) in names.iter().zip(values) {
                    let Some((path, base_start)) = path_of(self.source, value) else {
                        continue;
                    };
                    if path.len() < 2 {
                        continue;
                    }
                    self.aliases.push(AliasCandidate {
                        name: self.source.text_of(name).to_owned(),
                        name_start: name.start,
                        path,
                        base_start,
                        statement_end: statement.range.end,
                    });
                }
            }
            StatementKind::Assignment { targets, .. } => {
                for target in targets {
                    if let Some((path, _)) = path_of(self.source, target) {
                        self.assigned.push(path);
                    }
                    self.targets.push(target.range.clone());
                }
            }
            StatementKind::Function { name, .. } => {
                let mut path: Vec<String> = name
                    .path
                    .iter()
                    .map(|token| self.source.text_of(token).to_owned())
                    .collect();
                if let Some(method) = &name.method {
                    path.push(self.source.text_of(method).to_owned());
                }
                self.assigned.push(path);
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        if matches!(expression.kind, ExpressionKind::Field { .. })
            && let Some((path, _)) = path_of(self.source, expression)
        {
            self.uses.push(PathUse {
                path,
                range: expression.range.clone(),
            });
        }
    }
}

inventory::submit! { Registration(|| Box::new(PreferredLocalAlias)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/PreferredLocalAlias";

    fn message(alias: &str, path: &str) -> String {
        format!("Use the local alias `{alias}` instead of `{path}`.")
    }

    #[test]
    fn flags_paths_with_an_alias_in_scope() {
        expect_offense(
            READER,
            "local insert = table.insert\ntable.insert(t, 1)\n",
            2,
            1,
            &message("insert", "table.insert"),
        );
        expect_offenses(
            READER,
            "local Panels = Flux.UI.Panels\nlocal function f()\n  return Flux.UI.Panels.Card, \
             x(Flux.UI.Panels:get())\nend\n",
            &[
                (3, 10, &message("Panels", "Flux.UI.Panels")),
                (3, 33, &message("Panels", "Flux.UI.Panels")),
            ],
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "local insert = table.insert\ntable.insert(t, 1)\nlocal x = table.insert.y\n",
            "local insert = table.insert\ninsert(t, 1)\nlocal x = insert.y\n",
        );
        expect_correction(
            READER,
            "local fmt = string.format\nlocal again = string.format\nprint(string.format('%d', 1))\n",
            "local fmt = string.format\nlocal again = fmt\nprint(again('%d', 1))\n",
        );
    }

    #[test]
    fn ignores_uses_before_or_outside_the_alias() {
        expect_no_offenses(
            READER,
            "table.insert(t, 1)\nlocal insert = table.insert\ninsert(t, 2)\n",
        );
        expect_no_offenses(
            READER,
            "do local insert = table.insert end\ntable.insert(t, 1)\n",
        );
        expect_no_offenses(READER, "local a, b = table.insert\ntable.remove(t)\n");
    }

    #[test]
    fn ignores_shadowed_aliases_and_rebound_bases() {
        expect_no_offenses(
            READER,
            "local insert = table.insert\nlocal function f(insert)\n  table.insert(t, insert)\nend\n",
        );
        expect_no_offenses(
            READER,
            "local insert = table.insert\nlocal function f(table)\n  table.insert(1)\nend\n",
        );
        expect_no_offenses(
            READER,
            "local t = {}\nlocal i = t.insert\nlocal function f()\n  local t = {}\n  t.insert(1)\nend\n",
        );
        expect_no_offenses(
            READER,
            "local insert = table.insert\nlocal insert2 = 1 local insert = 2\ntable.insert(1)\n",
        );
    }

    #[test]
    fn ignores_reassigned_aliases_and_paths() {
        expect_no_offenses(
            READER,
            "local insert = table.insert\ninsert = nil\ntable.insert(t, 1)\n",
        );
        expect_no_offenses(
            READER,
            "local insert = table.insert\ntable.insert = my_insert\ntable.insert(t, 1)\n",
        );
        expect_no_offenses(
            READER,
            "local insert = table.insert\nfunction table.insert() end\ntable.insert(t, 1)\n",
        );
        expect_no_offenses(
            READER,
            "local insert = table.insert\ntable = {}\ntable.insert(t, 1)\n",
        );
    }

    #[test]
    fn ignores_chains_not_starting_at_the_base() {
        expect_no_offenses(
            READER,
            "local insert = table.insert\nx.table.insert(t, 1)\nlocal y = table\n",
        );
    }

    #[test]
    fn copies_of_local_data_and_nested_locals_are_not_aliases() {
        expect_no_offenses(
            READER,
            "local data = {}\nlocal name = data.name\nprint(data.name)\n",
        );
        expect_no_offenses(
            READER,
            "function PANEL:Think()\n  local serial = self.serial\n  print(self.serial)\nend\n",
        );
        expect_no_offenses(
            READER,
            "local function f()\n  local insert = table.insert\n  table.insert(t, 1)\nend\n",
        );
        expect_no_offenses(
            READER,
            "if CLIENT then\n  local insert = table.insert\n  table.insert(t, 1)\nend\n",
        );
    }

    #[test]
    fn plain_names_are_not_aliases() {
        expect_no_offenses(READER, "local print = print\nprint(1)\n");
    }

    #[test]
    fn reports_without_fix_when_the_path_is_spread_out() {
        let src = "local insert = table.insert\ntable . insert(t, 1)\n";
        let offenses = inspect(READER, src);
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
        assert_eq!(autocorrect(READER, src), src);
    }

    #[test]
    fn checks_lua_spliced_into_markup() {
        expect_correction_lumen(
            READER,
            "local Format = Flux.Format\nreturn <text>{Flux.Format(1)}</text>\n",
            "local Format = Flux.Format\nreturn <text>{Format(1)}</text>\n",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(
            READER,
            "local insert = table.insert\ntable.insert(1)\nif x then\n",
        );
    }
}
