//! `Performance/GetAllLoop`.

use std::ops::Range;

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    ArgumentStyle, Expression, Statement, StatementKind, Visitor, parse, walk,
};
use crate::source::Source;

/// The functions building a fresh table of everything, and the iterators over a cached one.
const DEFAULT_ITERATORS: &[(&str, &str)] = &[
    ("player.GetAll", "player.Iterator"),
    ("ents.GetAll", "ents.Iterator"),
];

/// Flags generic `for` loops over the table a function in `Iterators` builds
/// (`for _, ply in ipairs(player.GetAll()) do`, `pairs(...)` or `next, player.GetAll()`), when the
/// table is used for nothing else: the matching iterator (`player.Iterator()`) walks a cached
/// table instead of building a new one on every call. The fix replaces the loop's iterator
/// expressions, unless a comment lies among them. Files that do not parse are skipped.
pub struct GetAllLoop;

impl Reader for GetAllLoop {
    fn name(&self) -> &'static str {
        "Performance/GetAllLoop"
    }

    fn description(&self) -> &'static str {
        "Checks for loops over `player.GetAll()` and `ents.GetAll()` that `player.Iterator()` and `ents.Iterator()` can replace."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let mut iterators = Hash::new();
        for (function, iterator) in DEFAULT_ITERATORS {
            iterators.insert(
                Yaml::String((*function).to_owned()),
                Yaml::String((*iterator).to_owned()),
            );
        }
        vec![("Iterators", Yaml::Hash(iterators))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            iterators: ctx.option_str_map("Iterators"),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for (range, message, replacement) in finder.found {
            if source.has_comment_in(&range) {
                ctx.add_offense(range, message);
            } else {
                ctx.add_offense_with_fix(
                    range.clone(),
                    message,
                    vec![Edit::replace(range, replacement)],
                );
            }
        }
    }
}

/// Collects the loops over a table that an iterator could walk instead.
struct Finder<'a> {
    source: &'a Source,
    iterators: Vec<(String, String)>,
    found: Vec<(Range<usize>, String, String)>,
}

impl Finder<'_> {
    /// How a loop with the iterator expressions `values` walks a table (`ipairs`, `pairs` or
    /// `next`) and the table expression, when it is `ipairs(t)`, `pairs(t)` or `next, t`.
    fn walked<'t>(&self, values: &'t [Expression]) -> Option<(&'static str, &'t Expression)> {
        match values {
            [call] => {
                let (function, arguments) = call.unparenthesized().as_call(self.source)?;
                let walker = match function.as_str() {
                    "ipairs" => "ipairs",
                    "pairs" => "pairs",
                    _ => return None,
                };
                match arguments.values.as_slice() {
                    [table] => Some((walker, table)),
                    _ => None,
                }
            }
            [next, table] => (next.dotted_name(self.source).as_deref() == Some("next"))
                .then_some(("next", table)),
            _ => None,
        }
    }

    /// The function and its iterator when `table` is a call without arguments of a function in
    /// `Iterators`.
    fn iterator(&self, table: &Expression) -> Option<(&str, &str)> {
        let (function, arguments) = table.unparenthesized().as_call(self.source)?;
        if arguments.style != ArgumentStyle::Parenthesized || !arguments.values.is_empty() {
            return None;
        }
        self.iterators
            .iter()
            .find(|(name, _)| *name == function)
            .map(|(name, iterator)| (name.as_str(), iterator.as_str()))
    }
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::GenericFor { values, .. } = &statement.kind else {
            return;
        };
        let Some((walker, table)) = self.walked(values) else {
            return;
        };
        let Some((function, iterator)) = self.iterator(table) else {
            return;
        };
        let (Some(first), Some(last)) = (values.first(), values.last()) else {
            return;
        };
        let range = first.range.start..last.range.end;
        let written = if walker == "next" {
            format!("next, {function}()")
        } else {
            format!("{walker}({function}())")
        };
        let message = format!("Use `{iterator}()` instead of `{written}`.");
        self.found.push((range, message, format!("{iterator}()")));
    }
}

inventory::submit! { Registration(|| Box::new(GetAllLoop)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Performance/GetAllLoop";

    #[test]
    fn flags_loops_over_get_all() {
        expect_offense(
            READER,
            "for k, v in ipairs(player.GetAll()) do print(v) end\n",
            1,
            13,
            "Use `player.Iterator()` instead of `ipairs(player.GetAll())`.",
        );
        expect_offense(
            READER,
            "for _, ent in pairs(ents.GetAll()) do end\n",
            1,
            15,
            "Use `ents.Iterator()` instead of `pairs(ents.GetAll())`.",
        );
        expect_offense(
            READER,
            "for _, ply in next, player.GetAll() do end\n",
            1,
            15,
            "Use `player.Iterator()` instead of `next, player.GetAll()`.",
        );
    }

    #[test]
    fn corrects_to_iterators() {
        expect_correction(
            READER,
            "for i, ply in ipairs(player.GetAll()) do\n  for _, e in pairs((ents.GetAll())) do end\nend\n",
            "for i, ply in player.Iterator() do\n  for _, e in ents.Iterator() do end\nend\n",
        );
        expect_correction(
            READER,
            "hook.Add('Think', 'x', function()\n  for _, p in next, player.GetAll() do end\nend)\n",
            "hook.Add('Think', 'x', function()\n  for _, p in player.Iterator() do end\nend)\n",
        );
    }

    #[test]
    fn keeps_comments() {
        let src = "for _, p in ipairs(--[[all]] player.GetAll()) do end\n";
        assert_eq!(autocorrect(READER, src), src);
        assert!(!inspect(READER, src)[0].correctable());
    }

    #[test]
    fn ignores_other_uses() {
        expect_no_offenses(
            READER,
            "local all = player.GetAll()\nfor _, p in ipairs(all) do end\n\
             for _, p in ipairs(player.GetHumans()) do end\n\
             for _, p in ipairs(player.GetAll(), 1) do end\n\
             for _, p in ipairs(team.GetPlayers(1)) do end\n\
             for _, p in next, player.GetAll(), nil do end\n\
             for i = 1, #player.GetAll() do end\n\
             for _, p in player.Iterator() do end\nprint(#player.GetAll())\n",
        );
    }

    #[test]
    fn iterators_are_configurable() {
        let offenses = inspect_with(
            READER,
            "for _, p in ipairs(player.GetHumans()) do end\nfor _, p in ipairs(player.GetAll()) do end\n",
            "Iterators: { player.GetHumans: humans.Iterator }",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Use `humans.Iterator()` instead of `ipairs(player.GetHumans())`."
        );
    }
}
