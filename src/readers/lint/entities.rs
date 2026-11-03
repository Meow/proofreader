//! Which variables hold an entity, shared by the readers about entities that may have been
//! removed (`Lint/UncheckedEntityInCallback`, `Lint/NullEntityCheck`).
//!
//! Proofreader knows no types, so a variable holds an entity by convention: `self` in a method
//! of one of the `EntityTables`, a variable named one of the `EntityNames`, or a local
//! initialized from a call of one of the `EntityFunctions`. A local initialized from a call of
//! one of the `AlwaysValidFunctions` never does, whatever its name. Each reader using this
//! module has these four options with the same defaults.

use std::collections::HashSet;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, yaml_strings};
use crate::readers::lint::ast::{Expression, ExpressionKind, Statement, StatementKind, Visitor};
use crate::readers::lint::scopes::Resolution;
use crate::readers::naming::declarations::DeclarationKind;
use crate::source::Source;
use crate::token::Token;

/// Tables whose methods' `self` is an entity or a panel.
pub const DEFAULT_ENTITY_TABLES: &[&str] = &["ENT", "SWEP", "EFFECT", "PANEL", "TOOL"];

/// Names of locals and parameters that hold an entity by convention.
pub const DEFAULT_ENTITY_NAMES: &[&str] = &[
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
pub const DEFAULT_ENTITY_FUNCTIONS: &[&str] = &[
    "ents.Create",
    "ents.CreateClientProp",
    "Entity",
    "player.GetBySteamID",
    "player.GetBySteamID64",
    "vgui.Create",
];

/// Functions returning an entity that stays valid, such as the local player.
pub const DEFAULT_ALWAYS_VALID_FUNCTIONS: &[&str] = &["LocalPlayer"];

/// The options naming entities, with their defaults, in display order.
pub fn entity_options() -> Vec<(&'static str, Yaml)> {
    vec![
        ("EntityTables", yaml_strings(DEFAULT_ENTITY_TABLES)),
        ("EntityNames", yaml_strings(DEFAULT_ENTITY_NAMES)),
        ("EntityFunctions", yaml_strings(DEFAULT_ENTITY_FUNCTIONS)),
        (
            "AlwaysValidFunctions",
            yaml_strings(DEFAULT_ALWAYS_VALID_FUNCTIONS),
        ),
    ]
}

/// What one walk over a file finds out about entities: walk the file with it as the
/// [`Visitor`], then ask [`Entities::is_entity`] and [`Entities::checked`].
pub struct Entities<'t> {
    source: &'t Source,
    entity_tables: Vec<String>,
    entity_names: Vec<String>,
    entity_functions: Vec<String>,
    valid_functions: Vec<String>,
    /// The `function` keywords (by offset) of the methods and fields of `entity_tables`.
    entity_methods: HashSet<usize>,
    /// The name tokens (by offset) of locals initialized from an entity function.
    entity_locals: HashSet<usize>,
    /// The name tokens (by offset) of locals initialized from an always valid function.
    valid_locals: HashSet<usize>,
    /// The name tokens checked with `IsValid(name)` or `name:IsValid()`, in source order.
    checks: Vec<Token>,
}

impl<'t> Entities<'t> {
    /// Reads the entity options of the running reader.
    pub fn new(source: &'t Source, ctx: &Context) -> Self {
        Entities {
            source,
            entity_tables: ctx.option_str_list("EntityTables", DEFAULT_ENTITY_TABLES),
            entity_names: ctx.option_str_list("EntityNames", DEFAULT_ENTITY_NAMES),
            entity_functions: ctx.option_str_list("EntityFunctions", DEFAULT_ENTITY_FUNCTIONS),
            valid_functions: ctx
                .option_str_list("AlwaysValidFunctions", DEFAULT_ALWAYS_VALID_FUNCTIONS),
            entity_methods: HashSet::new(),
            entity_locals: HashSet::new(),
            valid_locals: HashSet::new(),
            checks: Vec::new(),
        }
    }

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

    /// Whether the variable with index `variable` holds an entity.
    pub fn is_entity(&self, resolution: &Resolution, variable: usize) -> bool {
        let declared = &resolution.variables[variable];
        if self.valid_locals.contains(&declared.token.start) {
            return false;
        }
        match declared.kind {
            DeclarationKind::Parameter
                if declared.name == "self"
                    && declared
                        .function
                        .is_some_and(|keyword| self.entity_methods.contains(&keyword.start)) =>
            {
                true
            }
            DeclarationKind::Local if self.entity_locals.contains(&declared.token.start) => true,
            DeclarationKind::Local | DeclarationKind::Parameter | DeclarationKind::ForVariable => {
                self.entity_names.contains(&declared.name)
            }
            DeclarationKind::LocalFunction => false,
        }
    }

    /// Whether the variable with index `variable` is checked with `IsValid(name)` or
    /// `name:IsValid()` inside `range`.
    pub fn checked(&self, resolution: &Resolution, variable: usize, range: Range<usize>) -> bool {
        self.checks.iter().any(|token| {
            range.contains(&token.start)
                && resolution
                    .reference_at(token.start)
                    .and_then(|reference| reference.variable)
                    == Some(variable)
        })
    }
}

impl<'t> Visitor<'t> for Entities<'t> {
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
                if expression
                    .as_call(self.source)
                    .is_some_and(|(name, _)| name == "IsValid")
                    && let [argument] = arguments.values.as_slice()
                    && let ExpressionKind::Name(token) = &argument.unparenthesized().kind
                {
                    self.checks.push(*token);
                }
            }
            ExpressionKind::MethodCall { object, name, .. }
            | ExpressionKind::SafeMethodCall { object, name, .. } => {
                if let ExpressionKind::Name(token) = &object.unparenthesized().kind
                    && self.source.text_of(name) == "IsValid"
                {
                    self.checks.push(*token);
                }
            }
            _ => {}
        }
    }
}
