//! `Security/UncheckedConsoleCommand`.

use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::security::realm::server_handlers;
use crate::source::Source;
use crate::token::TokenKind;

/// The functions a player should not be able to run at will on the server.
const DEFAULT_PRIVILEGED_FUNCTIONS: &[&str] = &[
    "RunConsoleCommand",
    "game.ConsoleCommand",
    "RunString",
    "RunStringEx",
    "CompileString",
    "file.Write",
    "file.Append",
    "file.Delete",
    "ents.Create",
    "game.CleanUpMap",
];

/// The methods a player should not be able to call at will on the server.
const DEFAULT_PRIVILEGED_METHODS: &[&str] = &[
    "Kick",
    "Ban",
    "SetUserGroup",
    "SetHealth",
    "SetArmor",
    "Give",
    "StripWeapons",
    "GodEnable",
];

/// The permission checks: methods (and global functions) for the plain names, functions for the
/// dotted ones.
const DEFAULT_PERMISSION_CHECKS: &[&str] = &[
    "IsAdmin",
    "IsSuperAdmin",
    "IsUserGroup",
    "CheckGroup",
    "HasPermission",
    "IsListenServerHost",
    "query",
    "CAMI.PlayerHasAccess",
    "ULib.ucl.query",
];

/// Longest receiver text quoted in a message.
const MAX_QUOTED_RECEIVER: usize = 30;

/// Flags `concommand.Add(name, callback)` when the callback may run on the server, calls
/// something privileged and never checks the caller's permissions. Privileged calls are calls
/// of the `PrivilegedFunctions` (`RunConsoleCommand`, `game.ConsoleCommand`, `RunString`,
/// `file.Write`, `ents.Create`, ...) and of methods named in `PrivilegedMethods` (`Kick`,
/// `Ban`, `SetUserGroup`, `Give`, ...). A permission check is a call, anywhere in the
/// callback, of one of the `PermissionChecks`: a dotted entry (`CAMI.PlayerHasAccess`) matches a
/// call of that function, a plain entry (`IsAdmin`) a method call by that name or a call of
/// that global function. Privileged calls that only run for the server console are exempt:
/// those inside `if not IsValid(ply) then` (also `!ply:IsValid()`, `ply == NULL`) or after
/// `if IsValid(ply) then return end`.
///
/// The callback is a function literal or a function of the file passed by name; closures in it
/// count. Commands registered in client-only code are exempt, as for `Security/NetReadTable`.
/// Disabled by default: most commands are harmless and many servers check permissions in a
/// shared helper. Files that do not parse are skipped.
pub struct UncheckedConsoleCommand;

impl Reader for UncheckedConsoleCommand {
    fn name(&self) -> &'static str {
        "Security/UncheckedConsoleCommand"
    }

    fn description(&self) -> &'static str {
        "Checks for console commands that do something privileged on the server without checking permissions."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            (
                "PrivilegedFunctions",
                yaml_strings(DEFAULT_PRIVILEGED_FUNCTIONS),
            ),
            (
                "PrivilegedMethods",
                yaml_strings(DEFAULT_PRIVILEGED_METHODS),
            ),
            ("PermissionChecks", yaml_strings(DEFAULT_PERMISSION_CHECKS)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let functions = ctx.option_str_list("PrivilegedFunctions", DEFAULT_PRIVILEGED_FUNCTIONS);
        let methods = ctx.option_str_list("PrivilegedMethods", DEFAULT_PRIVILEGED_METHODS);
        let checks = ctx.option_str_list("PermissionChecks", DEFAULT_PERMISSION_CHECKS);
        let mut found = Vec::new();
        for handler in server_handlers(&block, source, "concommand.Add") {
            let mut finder = Finder {
                source,
                functions: &functions,
                methods: &methods,
                checks: &checks,
                privileged: Vec::new(),
                console_only: Vec::new(),
                checked: false,
            };
            walk(&handler.function.body, &mut finder);
            if finder.checked {
                continue;
            }
            let Some((_, privileged)) = finder.privileged.iter().find(|(offset, _)| {
                !finder
                    .console_only
                    .iter()
                    .any(|range| range.contains(offset))
            }) else {
                continue;
            };
            let ExpressionKind::Call { callee, .. } = &handler.call.kind else {
                continue;
            };
            let command = handler
                .name(source)
                .map_or_else(String::new, |name| format!(" `{name}`"));
            found.push((
                callee.range.clone(),
                format!(
                    "Console command{command} calls `{privileged}` without checking the caller's permissions."
                ),
            ));
        }
        found.sort_by_key(|(range, _)| range.start);
        for (range, message) in found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the privileged calls of a callback, the code that only runs for the server
/// console, and whether a permission check is made.
struct Finder<'a> {
    source: &'a Source,
    functions: &'a [String],
    methods: &'a [String],
    checks: &'a [String],
    /// The privileged calls as `(offset, description)`.
    privileged: Vec<(usize, String)>,
    console_only: Vec<Range<usize>>,
    checked: bool,
}

impl Finder<'_> {
    /// Whether `expression` is `IsValid(x)` or `x:IsValid()`.
    fn is_valid_check(&self, expression: &Expression) -> bool {
        let expression = expression.unparenthesized();
        match &expression.kind {
            ExpressionKind::Call { .. } => expression
                .as_call(self.source)
                .is_some_and(|(name, _)| name == "IsValid"),
            ExpressionKind::MethodCall { name, .. } => self.source.text_of(name) == "IsValid",
            _ => false,
        }
    }

    /// Whether `condition` only holds without a calling player: `not IsValid(x)`,
    /// `!x:IsValid()`, `x == NULL`.
    fn is_console_condition(&self, condition: &Expression) -> bool {
        match &condition.unparenthesized().kind {
            ExpressionKind::Unary { operator, operand } => {
                matches!(operator.kind, TokenKind::Not | TokenKind::Bang)
                    && self.is_valid_check(operand)
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                operator.kind == TokenKind::Eq
                    && [left, right]
                        .iter()
                        .any(|side| side.dotted_name(self.source).as_deref() == Some("NULL"))
            }
            _ => false,
        }
    }

    /// Whether a call of the function or method `name` is a permission check: a method name
    /// only ever matches a plain entry, a dotted function name only a dotted one.
    fn is_check(&self, name: &str) -> bool {
        self.checks.iter().any(|check| check == name)
    }
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn enter_block(&mut self, block: &'t Block) {
        for statement in &block.statements {
            if let StatementKind::If {
                clauses,
                otherwise: None,
                ..
            } = &statement.kind
                && let [clause] = clauses.as_slice()
                && self.is_valid_check(&clause.condition)
                && matches!(
                    clause.body.statements.last().map(|last| &last.kind),
                    Some(StatementKind::Return { .. })
                )
            {
                self.console_only.push(statement.range.end..block.range.end);
                break;
            }
        }
    }

    fn statement(&mut self, statement: &'t Statement) {
        if let StatementKind::If { clauses, .. } = &statement.kind {
            for clause in clauses {
                if self.is_console_condition(&clause.condition) {
                    self.console_only.push(clause.body.range.clone());
                }
            }
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Call { callee, .. } => {
                let Some(name) = callee.dotted_name(self.source) else {
                    return;
                };
                if self.is_check(&name) {
                    self.checked = true;
                } else if self.functions.contains(&name) {
                    self.privileged.push((expression.range.start, name));
                }
            }
            ExpressionKind::MethodCall { object, name, .. } => {
                let name = self.source.text_of(name);
                if self.is_check(name) {
                    self.checked = true;
                } else if self.methods.iter().any(|method| method == name) {
                    let receiver = &self.source.text[object.range.clone()];
                    let shown = if receiver.len() <= MAX_QUOTED_RECEIVER && !receiver.contains('\n')
                    {
                        format!("{receiver}:{name}")
                    } else {
                        format!(":{name}")
                    };
                    self.privileged.push((expression.range.start, shown));
                }
            }
            _ => {}
        }
    }
}

inventory::submit! { Registration(|| Box::new(UncheckedConsoleCommand)) }

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::Config;
    use crate::testing::*;

    const READER: &str = "Security/UncheckedConsoleCommand";

    /// The message for a command calling `call` unchecked.
    fn message(command: &str, call: &str) -> String {
        format!(
            "Console command{command} calls `{call}` without checking the caller's permissions."
        )
    }

    #[test]
    fn is_disabled_by_default() {
        let config = Config::defaults(Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn flags_unchecked_privileged_commands() {
        expect_offenses(
            READER,
            "concommand.Add('kickall', function(ply, cmd, args)\n\
             for _, target in player.Iterator() do target:Kick('bye') end\nend)\n\
             local function restart() RunConsoleCommand('changelevel', game.GetMap()) end\n\
             concommand.Add('restart', restart)\n\
             concommand.Add(name, function(ply) timer.Simple(1, function() ply:Give('weapon_rpg') end) end)\n",
            &[
                (1, 1, &message(" `kickall`", "target:Kick")),
                (5, 1, &message(" `restart`", "RunConsoleCommand")),
                (6, 1, &message("", "ply:Give")),
            ],
        );
    }

    #[test]
    fn accepts_permission_checks() {
        expect_no_offenses(
            READER,
            "concommand.Add('a', function(ply) if !ply:IsAdmin() then return end RunString(x) end)\n\
             concommand.Add('b', function(ply) if CAMI.PlayerHasAccess(ply, 'b') then ply:SetPos(p) end end)\n\
             concommand.Add('c', function(ply) if ply:query('ulx c') then game.CleanUpMap() end end)\n\
             concommand.Add('d', function(ply) if IsSuperAdmin(ply) then game.CleanUpMap() end end)\n",
        );
    }

    #[test]
    fn accepts_console_only_code() {
        expect_no_offenses(
            READER,
            "concommand.Add('a', function(ply)\n  if !IsValid(ply) then RunConsoleCommand('bot') end\nend)\n\
             concommand.Add('b', function(ply)\n  if ply:IsValid() then return end\n  game.CleanUpMap()\nend)\n\
             concommand.Add('c', function(ply)\n  if ply == NULL then file.Delete('x.txt') end\nend)\n",
        );
        expect_offense(
            READER,
            "concommand.Add('a', function(ply)\n  if !IsValid(ply) then return end\n  game.CleanUpMap()\nend)\n",
            1,
            1,
            &message(" `a`", "game.CleanUpMap"),
        );
    }

    #[test]
    fn accepts_harmless_and_client_commands() {
        expect_no_offenses(
            READER,
            "concommand.Add('a', function(ply) ply:ChatPrint('hi') end)\n\
             concommand.Add('suicide', function(ply) ply:Kill() end)\n\
             if CLIENT then concommand.Add('b', function() RunConsoleCommand('say', 'hi') end) end\n",
        );
    }

    #[test]
    fn options() {
        let offenses = inspect_with(
            READER,
            "concommand.Add('a', function(ply) ply:Kill() end)\n\
             concommand.Add('b', function(ply) if ply:can('b') then ply:Kill() end end)\n\
             concommand.Add('c', function(ply) if perms.Check(ply) then ply:Kill() end end)\n\
             concommand.Add('d', function(ply) if obj:Check(ply) then ply:Kill() end end)\n",
            "PrivilegedMethods: [Kill]\nPermissionChecks: [can, perms.Check]",
        );
        let found: Vec<(u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, message(" `a`", "ply:Kill").as_str()),
                (4, message(" `d`", "ply:Kill").as_str()),
            ]
        );
    }
}
