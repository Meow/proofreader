//! `Naming/FieldName`.

use globset::{Glob, GlobSet, GlobSetBuilder};
use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, FieldKey, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::naming::case::NamingStyle;
use crate::source::Source;
use crate::token::Token;

/// Field names other code looks up and that keep their spelling: GMod's `m_` member fields
/// (`m_bJumping`), the colours and textures of Derma skins (`colButtonText`, `texGradientUp`) and
/// MySQLOO's callbacks.
const ALLOWED: [&str; 9] = [
    "m_*",
    "col[A-Z]*",
    "tex[A-Z]*",
    "onSuccess",
    "onError",
    "onAborted",
    "onData",
    "onConnected",
    "onConnectionFailed",
];

/// Checks the names of table fields the code defines against `EnforcedStyle`: assigned fields
/// (`self.raise_fraction = x`, `obj.on_done = function() end`) and the named keys of table
/// constructors (`{ max_size = 4 }`).
///
/// The styles judge names as `Naming/VariableName` does (`snake_case` flags lowerCamelCase).
/// Fields of tables named in all capitals (`SKIN.colButtonText`, `ENT.PrintName`, `TOOL.Name`)
/// keep the names the game looks up and are exempt, and so are the names matching `AllowedNames`,
/// glob patterns (`*`, `?`, `[A-Z]`) that default to GMod's `m_` member fields, Derma skin colours
/// and textures (`col[A-Z]*`, `tex[A-Z]*`) and MySQLOO's callbacks. Reading a field never counts, only defining one; functions
/// defined with `function a.b()` are left to `Naming/MethodName`. Files that do not parse are
/// skipped.
pub struct FieldName;

impl Reader for FieldName {
    fn name(&self) -> &'static str {
        "Naming/FieldName"
    }

    fn description(&self) -> &'static str {
        "Checks that the table fields the code defines follow the configured naming style."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("EnforcedStyle", Yaml::String("snake_case".to_owned())),
            (
                "AllowedNames",
                Yaml::Array(
                    ALLOWED
                        .into_iter()
                        .map(|name| Yaml::String(name.to_owned()))
                        .collect(),
                ),
            ),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let style = NamingStyle::from_option(ctx.option_str("EnforcedStyle", "snake_case"));
        let mut finder = Finder {
            source,
            style,
            allowed: allowed_names(&ctx.option_str_list("AllowedNames", &ALLOWED)),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        let message = format!("Use {} for field names.", style.label());
        for token in finder.found {
            ctx.add_offense(token.range(), message.clone());
        }
    }
}

/// Collects the field names that break the style.
struct Finder<'a> {
    source: &'a Source,
    style: NamingStyle,
    allowed: GlobSet,
    found: Vec<Token>,
}

/// The `AllowedNames` patterns as a glob set; a pattern that is not a valid glob matches only
/// itself.
fn allowed_names(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).or_else(|_| Glob::new(&globset::escape(pattern)));
        if let Ok(glob) = glob {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

impl Finder<'_> {
    /// Records `name` when it breaks the style and is not allowed.
    fn check(&mut self, name: Token) {
        let text = self.source.text_of(&name);
        if self.style.rejects(text) && !self.allowed.is_match(text) {
            self.found.push(name);
        }
    }
}

/// The name at the root of a field or index chain (`a` in `a.b[c].d`).
fn root_name<'a>(source: &'a Source, expression: &Expression) -> Option<&'a str> {
    match &expression.kind {
        ExpressionKind::Name(token) => Some(source.text_of(token)),
        ExpressionKind::Field { object, .. } | ExpressionKind::Index { object, .. } => {
            root_name(source, object)
        }
        _ => None,
    }
}

/// Whether `name` is written in capitals (and digits and underscores), like `SKIN` or `GM`.
fn is_capitals(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::Assignment { targets, .. } = &statement.kind else {
            return;
        };
        for target in targets {
            if let ExpressionKind::Field { object, name } = &target.kind
                && !root_name(self.source, object).is_some_and(is_capitals)
            {
                self.check(*name);
            }
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        if let ExpressionKind::Table(table) = &expression.kind {
            for field in &table.fields {
                if let FieldKey::Named(name) = field.key {
                    self.check(name);
                }
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(FieldName)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Naming/FieldName";
    const MESSAGE: &str = "Use snake_case for field names.";

    #[test]
    fn flags_camel_case_fields_and_keys() {
        expect_offenses(
            READER,
            "self.curRaisedFrac = 1\nlocal t = { maxSize = 4, ok = true }\nobj.a.onDone = function() end\n",
            &[(1, 6, MESSAGE), (2, 13, MESSAGE), (3, 7, MESSAGE)],
        );
    }

    #[test]
    fn accepts_other_styles_and_reads() {
        expect_no_offenses(
            READER,
            "self.raise_fraction = 1\nt.PrintName = 'x'\nt.MAX = 1\nprint(obj.fooBar)\n\
             x = { [fooBar] = 1, ['fooBar'] = 2 }\n",
        );
    }

    #[test]
    fn game_tables_and_allowed_names_are_exempt() {
        expect_no_offenses(
            READER,
            "SKIN.colButtonText = c\nENT.renderGroup = 1\nquery.onSuccess = function() end\n\
             db.onConnectionFailed = f\nself.colPropertySheet = c\nply.m_bJumping = true\n",
        );
        let offenses = inspect_with(READER, "query.onSuccess = f\n", "AllowedNames: []");
        assert_eq!(offenses.len(), 1);
        assert!(
            inspect_with(
                READER,
                "t.fooBar = 1\nt.fooBaz = 2\n",
                "AllowedNames: ['foo*']"
            )
            .is_empty()
        );
    }

    #[test]
    fn function_definitions_are_left_to_method_name() {
        expect_no_offenses(
            READER,
            "function obj.doThing() end\nfunction obj:doThing() end\n",
        );
    }

    #[test]
    fn camel_case_style() {
        let offenses = inspect_with(
            READER,
            "t.max_size = 1\nt.maxSize = 2\n",
            "EnforcedStyle: camelCase",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, "Use camelCase for field names.");
    }
}
