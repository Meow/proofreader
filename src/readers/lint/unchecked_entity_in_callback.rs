//! `Lint/UncheckedEntityInCallback`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::source::Source;
use crate::token::Token;

/// The functions running a callback later, with the 1-based position of the callback argument.
const DEFAULT_FUNCTIONS: &[(&str, i64)] = &[("timer.Simple", 2), ("timer.Create", 4)];

/// Tables whose methods' `self` is an entity or a panel.
const DEFAULT_ENTITY_TABLES: &[&str] = &["ENT", "SWEP", "EFFECT", "PANEL", "TOOL"];

/// Names of locals and parameters that hold an entity by convention.
const DEFAULT_ENTITY_NAMES: &[&str] = &[
    "ply",
    "player",
    "pl",
    "client",
    "ent",
    "entity",
    "target",
    "victim",
    "attacker",
    "inflictor",
    "owner",
    "weapon",
    "wep",
    "npc",
    "vehicle",
    "ragdoll",
    "prop",
    "panel",
];

/// Functions returning an entity or a panel.
const DEFAULT_ENTITY_FUNCTIONS: &[&str] = &[
    "ents.Create",
    "ents.CreateClientProp",
    "Entity",
    "player.GetBySteamID",
    "player.GetBySteamID64",
    "vgui.Create",
];

/// Functions returning an entity that stays valid, such as the local player.
const DEFAULT_ALWAYS_VALID_FUNCTIONS: &[&str] = &["LocalPlayer"];

/// Flags method calls on a captured entity inside a deferred callback that does not check the
/// entity is still valid: by the time the callback runs the entity may have been removed (or the
/// player may have left), and the call raises "Tried to use a NULL entity!".
///
/// The callbacks are function literals passed to the functions in `Functions`, which maps each
/// function to the 1-based position of its callback argument; closures nested in a callback are
/// part of it. A call `e:Method(...)` is flagged when `e` is a local, parameter or loop variable
/// declared outside the callback that holds an entity: `self` in a method of one of the
/// `EntityTables` (`function ENT:Think()`, `function SWEP.Reload(self)`, `ENT.Use =
/// function(self)`), a variable whose name is one of the `EntityNames`, or a local initialized
/// from a call of one of the `EntityFunctions` (`local prop = ents.Create('prop_physics')`). A
/// local initialized from a call of one of the `AlwaysValidFunctions` (`local client =
/// LocalPlayer()`) is never flagged, whatever its name.
/// The call is not flagged when `IsValid(e)` or `e:IsValid()` appears anywhere in the callback
/// before it (by source position, whatever the condition around it), and `e:IsValid()` itself
/// is never flagged. Field accesses (`e.Field`) and passing `e` to functions are not checked.
/// Each variable is reported once per callback, at its first unchecked call. Files that do not
/// parse are skipped.
pub struct UncheckedEntityInCallback;

impl Reader for UncheckedEntityInCallback {
    fn name(&self) -> &'static str {
        "Lint/UncheckedEntityInCallback"
    }

    fn description(&self) -> &'static str {
        "Checks for deferred callbacks that use a captured entity without checking `IsValid`."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let mut functions = Hash::new();
        for (function, position) in DEFAULT_FUNCTIONS {
            functions.insert(
                Yaml::String((*function).to_owned()),
                Yaml::Integer(*position),
            );
        }
        vec![
            ("Functions", Yaml::Hash(functions)),
            ("EntityTables", yaml_strings(DEFAULT_ENTITY_TABLES)),
            ("EntityNames", yaml_strings(DEFAULT_ENTITY_NAMES)),
            ("EntityFunctions", yaml_strings(DEFAULT_ENTITY_FUNCTIONS)),
            (
                "AlwaysValidFunctions",
                yaml_strings(DEFAULT_ALWAYS_VALID_FUNCTIONS),
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
        let functions: Vec<(String, usize)> = ctx
            .option_str_map("Functions")
            .into_iter()
            .filter_map(|(name, position)| Some((name, position.trim().parse().ok()?)))
            .collect();
        let mut collector = Collector {
            source,
            functions,
            entity_tables: ctx.option_str_list("EntityTables", DEFAULT_ENTITY_TABLES),
            entity_functions: ctx.option_str_list("EntityFunctions", DEFAULT_ENTITY_FUNCTIONS),
            valid_functions: ctx
                .option_str_list("AlwaysValidFunctions", DEFAULT_ALWAYS_VALID_FUNCTIONS),
            entity_methods: HashSet::new(),
            entity_locals: HashSet::new(),
            valid_locals: HashSet::new(),
            callbacks: Vec::new(),
            method_calls: Vec::new(),
            checks: Vec::new(),
        };
        walk(&block, &mut collector);
        let checker = Checker {
            source,
            resolution: &resolution,
            collector: &collector,
            entity_names: ctx.option_str_list("EntityNames", DEFAULT_ENTITY_NAMES),
        };
        let mut found: Vec<(Range<usize>, String)> = Vec::new();
        for (callback, function) in &collector.callbacks {
            for (range, message) in checker.check(callback, function) {
                if !found.iter().any(|(seen, _)| *seen == range) {
                    found.push((range, message));
                }
            }
        }
        found.sort_by_key(|(range, _)| range.start);
        for (range, message) in found {
            ctx.add_offense(range, message);
        }
    }
}

/// A method call on a plain name: `name:method(...)`.
struct MethodCall {
    /// The receiver's name token.
    object: Token,
    /// The method name.
    method: Token,
}

/// What the reader needs from one walk over a file.
struct Collector<'t> {
    source: &'t Source,
    functions: Vec<(String, usize)>,
    entity_tables: Vec<String>,
    entity_functions: Vec<String>,
    valid_functions: Vec<String>,
    /// The `function` keywords (by offset) of the methods and fields of `entity_tables`.
    entity_methods: HashSet<usize>,
    /// The name tokens (by offset) of locals initialized from an entity function.
    entity_locals: HashSet<usize>,
    /// The name tokens (by offset) of locals initialized from an always valid function.
    valid_locals: HashSet<usize>,
    /// Every callback with the name of the function it is passed to.
    callbacks: Vec<(&'t Function, String)>,
    /// Every method call on a plain name, in source order.
    method_calls: Vec<MethodCall>,
    /// The name tokens checked with `IsValid(name)` or `name:IsValid()`.
    checks: Vec<Token>,
}

impl Collector<'_> {
    /// Whether `expression` is a name in `entity_tables`.
    fn is_entity_table(&self, expression: &Expression) -> bool {
        matches!(
            &expression.kind,
            ExpressionKind::Name(token) if self.entity_tables.iter().any(|table| table == self.source.text_of(token))
        )
    }

    /// The name of the function `expression` calls, when it is a call of a plain function.
    fn called(&self, expression: &Expression) -> Option<String> {
        Some(expression.unparenthesized().as_call(self.source)?.0)
    }
}

impl<'t> Visitor<'t> for Collector<'t> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Function { name, function } => {
                if let Some(table) = name.path.first()
                    && name.path.len() + usize::from(name.method.is_some()) == 2
                    && self
                        .entity_tables
                        .iter()
                        .any(|entity| entity == self.source.text_of(table))
                {
                    self.entity_methods.insert(function.keyword.start);
                }
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in targets.iter().zip(values) {
                    if let ExpressionKind::Field { object, .. } = &target.kind
                        && self.is_entity_table(object)
                        && let ExpressionKind::Function(function) = &value.unparenthesized().kind
                    {
                        self.entity_methods.insert(function.keyword.start);
                    }
                }
            }
            StatementKind::Local { names, values, .. } => {
                for (name, value) in names.iter().zip(values) {
                    let Some(function) = self.called(value) else {
                        continue;
                    };
                    if self.entity_functions.contains(&function) {
                        self.entity_locals.insert(name.start);
                    } else if self.valid_functions.contains(&function) {
                        self.valid_locals.insert(name.start);
                    }
                }
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Call { arguments, .. } => {
                let Some((name, _)) = expression.as_call(self.source) else {
                    return;
                };
                if name == "IsValid"
                    && let [argument] = arguments.values.as_slice()
                    && let ExpressionKind::Name(token) = &argument.unparenthesized().kind
                {
                    self.checks.push(*token);
                }
                let callback = self
                    .functions
                    .iter()
                    .find(|(function, _)| *function == name)
                    .and_then(|(_, position)| arguments.values.get(position.checked_sub(1)?));
                if let Some(callback) = callback
                    && let ExpressionKind::Function(function) = &callback.unparenthesized().kind
                {
                    self.callbacks.push((function, name));
                }
            }
            ExpressionKind::MethodCall { object, name, .. } => {
                if let ExpressionKind::Name(token) = &object.unparenthesized().kind {
                    if self.source.text_of(name) == "IsValid" {
                        self.checks.push(*token);
                    }
                    self.method_calls.push(MethodCall {
                        object: *token,
                        method: *name,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Checks the callbacks of a file.
struct Checker<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    collector: &'a Collector<'a>,
    entity_names: Vec<String>,
}

impl Checker<'_> {
    /// The offenses of `callback`, passed to `function`: the first unchecked method call on each
    /// captured entity.
    fn check(&self, callback: &Function, function: &str) -> Vec<(Range<usize>, String)> {
        let range = callback.keyword.start..callback.end.end;
        let mut reported = HashSet::new();
        let mut found = Vec::new();
        for call in &self.collector.method_calls {
            if !range.contains(&call.object.start) || self.source.text_of(&call.method) == "IsValid"
            {
                continue;
            }
            let Some(variable) = self.variable(&call.object) else {
                continue;
            };
            let declared = &self.resolution.variables[variable];
            if range.contains(&declared.token.start)
                || !self.is_entity(variable)
                || self.checked(variable, range.start..call.object.start)
                || !reported.insert(variable)
            {
                continue;
            }
            let name = &declared.name;
            let runs = if function.starts_with("timer.") {
                "this timer runs"
            } else {
                "this callback runs"
            };
            found.push((
                call.object.start..call.method.end,
                format!("`{name}` may be removed before {runs}; check `IsValid({name})` first."),
            ));
        }
        found
    }

    /// The local a name token resolves to.
    fn variable(&self, token: &Token) -> Option<usize> {
        self.resolution.reference_at(token.start)?.variable
    }

    /// Whether the variable with index `variable` holds an entity.
    fn is_entity(&self, variable: usize) -> bool {
        let declared = &self.resolution.variables[variable];
        if self.collector.valid_locals.contains(&declared.token.start) {
            return false;
        }
        match declared.kind {
            DeclarationKind::Parameter
                if declared.name == "self"
                    && declared.function.is_some_and(|keyword| {
                        self.collector.entity_methods.contains(&keyword.start)
                    }) =>
            {
                true
            }
            DeclarationKind::Local
                if self.collector.entity_locals.contains(&declared.token.start) =>
            {
                true
            }
            DeclarationKind::Local | DeclarationKind::Parameter | DeclarationKind::ForVariable => {
                self.entity_names.contains(&declared.name)
            }
            DeclarationKind::LocalFunction => false,
        }
    }

    /// Whether the variable with index `variable` is checked with `IsValid` inside `range`.
    fn checked(&self, variable: usize, range: Range<usize>) -> bool {
        self.collector
            .checks
            .iter()
            .any(|token| range.contains(&token.start) && self.variable(token) == Some(variable))
    }
}

inventory::submit! { Registration(|| Box::new(UncheckedEntityInCallback)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/UncheckedEntityInCallback";

    /// The message for `name` used in a timer.
    fn message(name: &str) -> String {
        format!("`{name}` may be removed before this timer runs; check `IsValid({name})` first.")
    }

    #[test]
    fn flags_entity_names() {
        expect_offense(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    ply:Kill()\n  end)\nend\n",
            3,
            5,
            &message("ply"),
        );
        expect_offense(
            READER,
            "for _, ent in ipairs(list) do\n  timer.Create('x', 1, 0, function() ent:Remove() end)\nend\n",
            2,
            38,
            &message("ent"),
        );
    }

    #[test]
    fn flags_self_in_entity_methods() {
        expect_offense(
            READER,
            "function ENT:Use()\n  timer.Simple(1, function()\n    self:SetColor(color)\n  end)\nend\n",
            3,
            5,
            &message("self"),
        );
        expect_offense(
            READER,
            "function SWEP.Reload(self)\n  timer.Simple(1, function() self:SetClip1(1) end)\nend\n",
            2,
            30,
            &message("self"),
        );
        expect_offense(
            READER,
            "PANEL.Init = function(self)\n  timer.Simple(1, function() self:SetVisible(true) end)\nend\n",
            2,
            30,
            &message("self"),
        );
    }

    #[test]
    fn flags_locals_from_entity_functions() {
        expect_offense(
            READER,
            "local prop = ents.Create('prop_physics')\ntimer.Simple(5, function() prop:Remove() end)\n",
            2,
            28,
            &message("prop"),
        );
        expect_offense(
            READER,
            "local thing = vgui.Create('DPanel')\ntimer.Simple(5, function() thing:Remove() end)\n",
            2,
            28,
            &message("thing"),
        );
    }

    #[test]
    fn flags_nested_closures_once_per_variable() {
        expect_offense(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    ply:Kill()\n    ply:Spawn()\n    \
             local function g() ply:Freeze(true) end\n  end)\nend\n",
            3,
            5,
            &message("ply"),
        );
        expect_offenses(
            READER,
            "local function f(ply, ent)\n  timer.Simple(1, function()\n    hook.Run('X', function() ent:Remove() end)\n    \
             ply:Kill()\n  end)\nend\n",
            &[(3, 30, &message("ent")), (4, 5, &message("ply"))],
        );
    }

    #[test]
    fn accepts_checked_entities() {
        expect_no_offenses(
            READER,
            "local function f(ply, ent)\n  timer.Simple(1, function()\n    if !IsValid(ply) then return end\n    ply:Kill()\n    \
             if ent:IsValid() then ent:Remove() end\n  end)\nend\n\
             function ENT:Use()\n  timer.Simple(1, function()\n    if IsValid(self) and self:GetOn() then self:Toggle() end\n  end)\nend\n",
        );
    }

    #[test]
    fn requires_the_check_in_the_callback_before_the_call() {
        expect_offense(
            READER,
            "local function f(ply)\n  if !IsValid(ply) then return end\n  timer.Simple(1, function() ply:Kill() end)\nend\n",
            3,
            30,
            &message("ply"),
        );
        expect_offense(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    ply:Kill()\n    if IsValid(ply) then end\n  end)\nend\n",
            3,
            5,
            &message("ply"),
        );
        expect_offense(
            READER,
            "local function f(ply, ent)\n  timer.Simple(1, function()\n    if IsValid(ent) then ply:Kill() end\n  end)\nend\n",
            3,
            26,
            &message("ply"),
        );
    }

    #[test]
    fn checks_inner_callbacks_separately() {
        expect_offense(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    if !IsValid(ply) then return end\n    \
             timer.Simple(1, function() ply:Kill() end)\n  end)\nend\n",
            4,
            32,
            &message("ply"),
        );
    }

    #[test]
    fn accepts_variables_declared_in_the_callback_and_other_values() {
        expect_no_offenses(
            READER,
            "timer.Simple(1, function()\n  local ply = player.GetAll()[1]\n  ply:Kill()\nend)\n\
             timer.Simple(1, function(ent) ent:Remove() end)\n\
             local function f(data, ply)\n  timer.Simple(1, function() data:Save() print(ply.name) end)\nend\n\
             function GM:Think()\n  timer.Simple(1, function() self:Update() end)\nend\n\
             function ENT:Use()\n  timer.Simple(1, function() ply:Kill() end)\nend\n\
             local client = LocalPlayer()\ntimer.Simple(1, function() client:ChatPrint('x') end)\n\
             local function f(ply)\n  hook.Add('Think', 'x', function() ply:Kill() end)\n  ply:Kill()\n  \
             timer.Simple(1, on_done)\nend\n",
        );
    }

    #[test]
    fn respects_shadowing() {
        expect_no_offenses(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    local ply = Entity(1)\n    ply:Kill()\n  end)\nend\n",
        );
        expect_offense(
            READER,
            "local function f(ply)\n  timer.Simple(1, function()\n    do local ply = 1 IsValid(ply) end\n    ply:Kill()\n  end)\nend\n",
            4,
            5,
            &message("ply"),
        );
    }

    #[test]
    fn honours_options() {
        let src = "local function f(thing)\n  defer(function() thing:Remove() end, 0)\n  \
                   timer.Simple(1, function() thing:Remove() end)\nend\n";
        expect_no_offenses(READER, src);
        let offenses = inspect_with(READER, src, "Functions: { defer: 1 }\nEntityNames: [thing]");
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [(
                2,
                20,
                "`thing` may be removed before this callback runs; check `IsValid(thing)` first."
            )]
        );
        assert!(
            inspect_with(
                READER,
                "function ENT:Use() timer.Simple(1, function() self:Remove() end) end\n",
                "EntityTables: [WEAPON]",
            )
            .is_empty()
        );
        assert_eq!(
            inspect_with(
                READER,
                "local x = MakeThing()\ntimer.Simple(1, function() x:Remove() end)\n",
                "EntityFunctions: [MakeThing]",
            )
            .len(),
            1
        );
        assert!(
            inspect_with(
                READER,
                "local ply = GetHost()\ntimer.Simple(1, function() ply:Kill() end)\n",
                "AlwaysValidFunctions: [GetHost]",
            )
            .is_empty()
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(
            READER,
            "local function f(ply) timer.Simple(1, function() ply:Kill() end)\n",
        );
    }
}
