//! `Lint/HookName`.

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;

/// The functions taking a hook name, with the 1-based position of the name argument.
const DEFAULT_FUNCTIONS: &[(&str, i64)] = &[
    ("hook.Add", 1),
    ("hook.Remove", 1),
    ("hook.Run", 1),
    ("hook.Call", 1),
    ("gamemode.Call", 1),
];

/// Flags hook names that can never name a hook: the name argument of a call of one of the
/// `Functions` (`hook.Add('Think', ...)`, `hook.Run('PlayerSpawn', ply)`) that is an empty or
/// blank string, a string with leading or trailing whitespace, or a literal that is not a string
/// (`hook.Run(123)`, `hook.Add(nil, ...)`, a table or a function). Such a hook never fires, or a
/// handler is registered for an event nobody runs.
///
/// Names built at run time (variables, fields, calls, concatenations) are left alone: hooks
/// added in a loop over event names or run from a configured name are common and correct. As
/// in `gmod-invalid-hook-name` of GLuaLS, only literals are judged. Files that do not parse are
/// skipped.
pub struct HookName;

impl Reader for HookName {
    fn name(&self) -> &'static str {
        "Lint/HookName"
    }

    fn description(&self) -> &'static str {
        "Checks for hook names that are empty, padded with whitespace or not a string."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let mut functions = Hash::new();
        for (function, position) in DEFAULT_FUNCTIONS {
            functions.insert(
                Yaml::String((*function).to_owned()),
                Yaml::Integer(*position),
            );
        }
        vec![("Functions", Yaml::Hash(functions))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let functions: Vec<(String, usize)> = ctx
            .option_str_map("Functions")
            .into_iter()
            .filter_map(|(name, position)| Some((name, position.trim().parse().ok()?)))
            .filter(|(_, position)| *position > 0)
            .collect();
        let mut finder = Finder {
            source,
            functions,
            found: Vec::new(),
        };
        walk(block, &mut finder);
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the bad hook names of a file.
struct Finder<'a> {
    source: &'a Source,
    functions: Vec<(String, usize)>,
    found: Vec<(std::ops::Range<usize>, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let Some((function, arguments)) = expression.as_call(self.source) else {
            return;
        };
        let Some((_, position)) = self.functions.iter().find(|(name, _)| *name == function) else {
            return;
        };
        let Some(argument) = arguments.values.get(position - 1) else {
            return;
        };
        let value = argument.unparenthesized();
        let message = match &value.kind {
            ExpressionKind::String(_) => {
                let Some(name) = value.string_contents(self.source) else {
                    return;
                };
                if name.trim().is_empty() {
                    "Hook name should not be empty.".to_owned()
                } else if name.trim() != name {
                    format!("Hook name `{name}` has leading or trailing whitespace.")
                } else {
                    return;
                }
            }
            ExpressionKind::Nil => "Hook name should be a string, not `nil`.".to_owned(),
            ExpressionKind::True | ExpressionKind::False => {
                "Hook name should be a string, not a boolean.".to_owned()
            }
            ExpressionKind::Number => "Hook name should be a string, not a number.".to_owned(),
            ExpressionKind::Table(_) => "Hook name should be a string, not a table.".to_owned(),
            ExpressionKind::Function(_) => {
                "Hook name should be a string, not a function.".to_owned()
            }
            _ => return,
        };
        self.found.push((argument.range.clone(), message));
    }
}

inventory::submit! { Registration(|| Box::new(HookName)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/HookName";

    #[test]
    fn flags_empty_and_blank_names() {
        expect_offenses(
            READER,
            "hook.Add('', 'id', function() end)\nhook.Run('  ')\nhook.Remove([[]], 'id')\n",
            &[
                (1, 10, "Hook name should not be empty."),
                (2, 10, "Hook name should not be empty."),
                (3, 13, "Hook name should not be empty."),
            ],
        );
    }

    #[test]
    fn flags_padded_names() {
        expect_offense(
            READER,
            "hook.Add('Think ', 'id', f)\n",
            1,
            10,
            "Hook name `Think ` has leading or trailing whitespace.",
        );
    }

    #[test]
    fn flags_literals_that_are_not_strings() {
        expect_offenses(
            READER,
            "hook.Run(123)\nhook.Call(nil, GAMEMODE)\ngamemode.Call(true)\n\
             hook.Add({}, 'id', f)\nhook.Add(function() end, 'id', f)\nhook.Run((1))\n",
            &[
                (1, 10, "Hook name should be a string, not a number."),
                (2, 11, "Hook name should be a string, not `nil`."),
                (3, 15, "Hook name should be a string, not a boolean."),
                (4, 10, "Hook name should be a string, not a table."),
                (5, 10, "Hook name should be a string, not a function."),
                (6, 10, "Hook name should be a string, not a number."),
            ],
        );
    }

    #[test]
    fn accepts_names_and_dynamic_names() {
        expect_no_offenses(
            READER,
            "hook.Add('Think', 'id', f)\nhook.Run(\"PlayerSpawn\", ply)\n\
             for _, event in ipairs(events) do hook.Add(event, 'id', f) end\n\
             hook.Run(name)\nhook.Run(prefix .. 'Changed')\nhook.Run(get_name())\n\
             hook.Run()\nhook.Add 'Think'\nother.Run(123)\n",
        );
    }

    #[test]
    fn honours_functions() {
        let offenses = inspect_with(
            READER,
            "plugin.call('')\nhook.Run('')\n",
            "Functions: { plugin.call: 1 }",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].line, 1);
        expect_no_offenses(READER, "event.fire(1, '')\n");
        let offenses = inspect_with(
            READER,
            "event.fire(1, '')\n",
            "Functions: { event.fire: 2 }",
        );
        assert_eq!(offenses.len(), 1);
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "hook.Run('')\nif then\n");
    }
}
