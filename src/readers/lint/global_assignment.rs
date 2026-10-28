//! `Lint/GlobalAssignment`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;
use crate::token::Token;

/// Globals that GMod and the common frameworks expect code to assign, often from inside a
/// loader function.
const ALLOWED_GLOBALS: [&str; 22] = [
    "GM",
    "GAMEMODE",
    "ENT",
    "SWEP",
    "EFFECT",
    "TOOL",
    "PANEL",
    "PLUGIN",
    "SCHEMA",
    "Schema",
    "ITEM",
    "FACTION",
    "CLASS",
    "ATTRIBUTE",
    "LANGUAGE",
    "CHAT_CLASS",
    "PACKAGE",
    "CMD",
    "ROLE",
    "THEME",
    "SKILL",
    "PLAYER",
];

/// Flags assignments that create or overwrite a global from inside a function body, which is
/// almost always a missing `local`.
///
/// The writes are assignment targets that are plain names resolving to no local (`count = 0`)
/// and `function name() end` statements with a single-name path; fields, indexes and methods
/// (`t.x = 1`, `function GM:Think()`) are not global writes. Exempt are:
///
/// - writes at file level, which are intended globals in GMod (`MyAddon = MyAddon or {}`),
///   unless `AllowFileLevel` is `false`, and in-function writes of a name that is also assigned
///   at file level in the same file;
/// - the names in `AllowedGlobals` (the scripted-class and framework tables such as `ENT`,
///   `SWEP`, `PLUGIN` and `ITEM`) and, with `AllowUppercase`, every name without lowercase
///   letters (`CW_CONVAR_LOG`), which by convention is a deliberate global;
/// - a global the function reads before or in the statement of its first write there
///   (`local old = can`, `if !sfs then sfs = ... end`, `n = n + 1`): the function expects it to
///   exist already.
///
/// Each global is reported once per function (the innermost one around the write), at its first
/// write there, and once at file level when `AllowFileLevel` is `false`. Files that do not parse
/// are skipped.
pub struct GlobalAssignment;

impl Reader for GlobalAssignment {
    fn name(&self) -> &'static str {
        "Lint/GlobalAssignment"
    }

    fn description(&self) -> &'static str {
        "Checks for assignments to global variables from inside functions."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("AllowFileLevel", Yaml::Boolean(true)),
            ("AllowUppercase", Yaml::Boolean(true)),
            ("AllowedGlobals", yaml_strings(&ALLOWED_GLOBALS)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let allow_file_level = ctx.option_bool("AllowFileLevel", true);
        let allow_uppercase = ctx.option_bool("AllowUppercase", true);
        let allowed: HashSet<String> = ctx
            .option_str_list("AllowedGlobals", &ALLOWED_GLOBALS)
            .into_iter()
            .collect();
        let source = ctx.source;
        let (Ok(chunk), Some(resolution)) = (parse(source), resolve(source)) else {
            return;
        };
        let mut finder = Finder {
            source,
            resolution: &resolution,
            functions: Vec::new(),
            writes: Vec::new(),
        };
        walk(&chunk, &mut finder);
        let file_level: HashSet<&str> = finder
            .writes
            .iter()
            .filter(|write| write.function.is_none())
            .map(|write| write.name)
            .collect();
        let mut seen: HashSet<(Option<usize>, &str)> = HashSet::new();
        for write in &finder.writes {
            let function = write.function.as_ref().map(|function| function.start);
            if !seen.insert((function, write.name))
                || allowed.contains(write.name)
                || (allow_uppercase && is_uppercase(write.name))
                || (allow_file_level
                    && (write.function.is_none() || file_level.contains(write.name)))
                || read_first(source, &resolution, write)
            {
                continue;
            }
            let message = match write.function {
                Some(_) => format!(
                    "Assignment to the global `{}` inside a function; declare it `local` or \
                     assign it at file level.",
                    write.name
                ),
                None => format!(
                    "Assignment to the global `{}`; declare it `local`.",
                    write.name
                ),
            };
            ctx.add_offense(write.token.range(), message);
        }
    }
}

/// Whether `name` has no lowercase letters and at least one uppercase one (`PLUGIN_BASE_DIR`).
fn is_uppercase(name: &str) -> bool {
    !name.chars().any(|c| c.is_ascii_lowercase()) && name.chars().any(|c| c.is_ascii_uppercase())
}

/// Whether the function around `write` reads the global it assigns before the end of the
/// statement of the write.
fn read_first(source: &Source, resolution: &Resolution, write: &Write) -> bool {
    let Some(function) = &write.function else {
        return false;
    };
    resolution.references.iter().any(|reference| {
        !reference.write
            && reference.variable.is_none()
            && (function.start..write.statement_end).contains(&reference.token.start)
            && source.text_of(&reference.token) == write.name
    })
}

/// A write of a global name.
struct Write<'a> {
    /// The assigned name.
    name: &'a str,
    /// Its `Name` token.
    token: Token,
    /// The end of the statement holding the write (for a `function name()` statement, the end
    /// of its `function` keyword).
    statement_end: usize,
    /// From the `function` keyword to the `end` of the innermost function around the write;
    /// `None` at file level.
    function: Option<Range<usize>>,
}

/// Collects the global writes in source order.
struct Finder<'a, 'r> {
    source: &'a Source,
    resolution: &'r Resolution<'a>,
    /// The ranges of the functions being walked, innermost last.
    functions: Vec<Range<usize>>,
    writes: Vec<Write<'a>>,
}

impl Finder<'_, '_> {
    /// Records the write of `token`, in a statement ending at `statement_end`, when it resolves
    /// to no local.
    fn record(&mut self, token: &Token, statement_end: usize) {
        let global = self
            .resolution
            .reference_at(token.start)
            .is_some_and(|reference| reference.write && reference.variable.is_none());
        if global {
            self.writes.push(Write {
                name: self.source.text_of(token),
                token: *token,
                statement_end,
                function: self.functions.last().cloned(),
            });
        }
    }
}

impl<'t> Visitor<'t> for Finder<'_, '_> {
    fn enter_function(&mut self, function: &'t Function) {
        self.functions
            .push(function.keyword.start..function.end.end);
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.functions.pop();
    }

    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Assignment { targets, .. } => {
                for target in targets {
                    if let ExpressionKind::Name(token) = &target.kind {
                        self.record(token, statement.range.end);
                    }
                }
            }
            StatementKind::CompoundAssignment { target, .. } => {
                if let ExpressionKind::Name(token) = &target.kind {
                    self.record(token, statement.range.end);
                }
            }
            StatementKind::Function { name, function } if name.method.is_none() => {
                if let [token] = name.path.as_slice() {
                    self.record(token, function.keyword.end);
                }
            }
            _ => {}
        }
    }
}

inventory::submit! { Registration(|| Box::new(GlobalAssignment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/GlobalAssignment";

    /// The message for a write of the global `name` inside a function.
    fn inside(name: &str) -> String {
        format!(
            "Assignment to the global `{name}` inside a function; declare it `local` or assign \
             it at file level."
        )
    }

    /// `(line, col, message)` of the offenses for `src` with `options`.
    fn found(src: &str, options: &str) -> Vec<(u32, u32, String)> {
        inspect_with(READER, src, options)
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect()
    }

    #[test]
    fn flags_global_writes_inside_functions() {
        expect_offense(
            READER,
            "local function f()\n  count = 0\nend\n",
            2,
            3,
            &inside("count"),
        );
        expect_offense(
            READER,
            "hook.Add('Think', 'x', function()\n  function helper() end\nend)\n",
            2,
            12,
            &inside("helper"),
        );
        expect_offense(
            READER,
            "function PANEL:Paint(w, h)\n  color = self.color\n  draw(color)\nend\n",
            2,
            3,
            &inside("color"),
        );
    }

    #[test]
    fn reports_each_global_once_per_function() {
        expect_offenses(
            READER,
            "function GM:Think()\n  a, b = 1, 2\n  a = a + 1\nend\n\
             function GM:Tick()\n  a = 0\n  local function g() a = 1 end\nend\n",
            &[
                (2, 3, &inside("a")),
                (2, 6, &inside("b")),
                (6, 3, &inside("a")),
                (7, 22, &inside("a")),
            ],
        );
    }

    #[test]
    fn accepts_locals_upvalues_fields_and_methods() {
        expect_no_offenses(
            READER,
            "local count\nlocal function f(x)\n  local y\n  count, x, y = 1, 2, 3\n\
             t.field = 1\n  t[1] = 2\nend\nfunction GM:Think() end\nfunction lib.helper() end\n",
        );
    }

    #[test]
    fn compound_assignments_are_writes_that_read_first() {
        expect_no_offenses(READER, "function f()\n  count += 1\nend\n");
        assert_eq!(
            found("count += 1\n", "AllowFileLevel: false"),
            vec![(
                1,
                1,
                "Assignment to the global `count`; declare it `local`.".to_owned()
            )]
        );
        expect_offense(
            READER,
            "local f = || -> do\n  total = 0\nend\n",
            2,
            3,
            &inside("total"),
        );
    }

    #[test]
    fn accepts_file_level_globals() {
        expect_no_offenses(
            READER,
            "MyAddon = MyAddon or {}\nfunction Helper() end\n\
             local function reset()\n  MyAddon = {}\n  function Helper() end\nend\n",
        );
    }

    #[test]
    fn accepts_globals_read_before_they_are_written() {
        expect_no_offenses(
            READER,
            "function load()\n  if !sfs then\n    sfs = include('sfs.lua')\n  end\nend\n\
             function swap()\n  local old = can\n  function can() end\n  run()\n  can = old\n\
             end\nfunction tick()\n  ticks = ticks + 1\nend\n",
        );
    }

    #[test]
    fn accepts_allowed_and_uppercase_globals() {
        expect_no_offenses(
            READER,
            "function load(path)\n  PLUGIN = {}\n  include(path)\n  PLUGIN = nil\n\
             CW_CONVAR_LOG = CreateClientConVar('cw_log', 1)\nend\n",
        );
        assert_eq!(
            found(
                "function load()\n  PLUGIN = {}\n  Registry = {}\nend\n",
                "AllowedGlobals: [Registry]\nAllowUppercase: false",
            ),
            [(2, 3, inside("PLUGIN"))]
        );
    }

    #[test]
    fn flags_file_level_writes_when_not_allowed() {
        assert_eq!(
            found(
                "MyAddon = {}\nMyAddon = MyAddon or {}\nfunction f() MyAddon = nil end\n\
                 ENT = {}\n",
                "AllowFileLevel: false",
            ),
            [
                (
                    1,
                    1,
                    "Assignment to the global `MyAddon`; declare it `local`.".to_owned()
                ),
                (
                    3,
                    10,
                    "Assignment to the global `f`; declare it `local`.".to_owned()
                ),
                (3, 14, inside("MyAddon")),
            ]
        );
    }

    #[test]
    fn checks_lumen() {
        expect_offense_lumen(
            READER,
            "return <view>{f(function() shown = true end)}</view>\n",
            1,
            28,
            &inside("shown"),
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "local function f() x = 1 end\nf(\n");
    }
}
