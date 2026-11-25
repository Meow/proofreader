//! `Lint/UncheckedEntityInCallback`.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Function, Statement, Visitor, parse, walk,
};
use crate::readers::lint::entities::{Entities, entity_options};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::source::Source;
use crate::token::Token;

/// The functions running a callback later, with the 1-based position of the callback argument.
const DEFAULT_FUNCTIONS: &[(&str, i64)] = &[("timer.Simple", 2), ("timer.Create", 4)];

/// Flags method calls on a captured entity inside a deferred callback that does not check the
/// entity is still valid: by the time the callback runs the entity may have been removed (or the
/// player may have left), and the call raises "Tried to use a NULL entity!".
///
/// The callbacks are function literals passed to the functions in `Functions`, which maps each
/// function to the 1-based position of its callback argument; closures nested in a callback are
/// part of it. A call `e:Method(...)` is flagged when `e` is a local, parameter or loop variable
/// declared outside the callback that holds an entity (see [`crate::readers::lint::entities`]):
/// `self` in a method of one of the `EntityTables` (`function ENT:Think()`, `function
/// SWEP.Reload(self)`, `ENT.Use = function(self)`), a variable whose name is one of the
/// `EntityNames`, or a local initialized from a call of one of the `EntityFunctions` (`local prop
/// = ents.Create('prop_physics')`). A local initialized from a call of one of the
/// `AlwaysValidFunctions` (`local client = LocalPlayer()`) is never flagged, whatever its name.
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
        let mut options = vec![("Functions", Yaml::Hash(functions))];
        options.extend(entity_options());
        options
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
            entities: Entities::new(source, ctx),
            callbacks: Vec::new(),
            method_calls: Vec::new(),
        };
        walk(block, &mut collector);
        let checker = Checker {
            source,
            resolution: &resolution,
            collector: &collector,
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
    /// Which variables hold an entity and where they are checked.
    entities: Entities<'t>,
    /// Every callback with the name of the function it is passed to.
    callbacks: Vec<(&'t Function, String)>,
    /// Every method call on a plain name, in source order.
    method_calls: Vec<MethodCall>,
}

impl<'t> Visitor<'t> for Collector<'t> {
    fn statement(&mut self, statement: &'t Statement) {
        self.entities.statement(statement);
    }

    fn expression(&mut self, expression: &'t Expression) {
        self.entities.expression(expression);
        match &expression.kind {
            ExpressionKind::Call { arguments, .. } => {
                let Some((name, _)) = expression.as_call(self.source) else {
                    return;
                };
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
            ExpressionKind::MethodCall { object, name, .. }
            | ExpressionKind::SafeMethodCall { object, name, .. } => {
                if let ExpressionKind::Name(token) = &object.unparenthesized().kind {
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
}

impl Checker<'_> {
    /// The offenses of `callback`, passed to `function`: the first unchecked method call on each
    /// captured entity.
    fn check(&self, callback: &Function, function: &str) -> Vec<(Range<usize>, String)> {
        let range = callback.keyword.start..callback.end.end;
        let entities = &self.collector.entities;
        let mut reported = HashSet::new();
        let mut found = Vec::new();
        for call in &self.collector.method_calls {
            if !range.contains(&call.object.start) || self.source.text_of(&call.method) == "IsValid"
            {
                continue;
            }
            let Some(variable) = self
                .resolution
                .reference_at(call.object.start)
                .and_then(|reference| reference.variable)
            else {
                continue;
            };
            let declared = &self.resolution.variables[variable];
            if range.contains(&declared.token.start)
                || !entities.is_entity(self.resolution, variable)
                || entities.checked(self.resolution, variable, range.start..call.object.start)
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
    fn short_functions_are_callbacks() {
        expect_offense(
            READER,
            "local function f(ent)\n  timer.Simple(1, || -> ent:Remove())\nend\n",
            2,
            25,
            &message("ent"),
        );
        expect_offense(
            READER,
            "local function f(ent)\n  timer.Simple(1, || -> do\n    ent?.:Remove()\n  end)\nend\n",
            3,
            5,
            &message("ent"),
        );
        expect_no_offenses(
            READER,
            "local function f(ent)\n  timer.Simple(1, || -> IsValid(ent) ? (ent:Remove()) : nil)\nend\n",
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
