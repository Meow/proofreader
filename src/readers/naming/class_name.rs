//! `Naming/ClassName`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, Visitor, parse, walk};
use crate::source::Source;

/// Checks that the class and module names given to Flux's `class 'Name'`, `mod 'Name'` and
/// `extends 'Name'` (the functions listed in `Functions`) are PascalCase.
///
/// Every `::` or `.` separated segment (`'Lumen::Compiler'`) must start with an upper case
/// letter and hold no underscore; acronyms (`'MVC'`, `'CPPI'`) are PascalCase. Only string
/// literal arguments are checked, and files that do not parse are skipped.
pub struct ClassName;

impl Reader for ClassName {
    fn name(&self) -> &'static str {
        "Naming/ClassName"
    }

    fn description(&self) -> &'static str {
        "Checks that class and module names use PascalCase."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![(
            "Functions",
            Yaml::Array(
                ["class", "mod", "extends"]
                    .into_iter()
                    .map(|name| Yaml::String(name.to_owned()))
                    .collect(),
            ),
        )]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            functions: ctx.option_str_list("Functions", &["class", "mod", "extends"]),
            found: Vec::new(),
        };
        walk(&block, &mut finder);
        for range in finder.found {
            ctx.add_offense(range, "Use PascalCase for class and module names.");
        }
    }
}

/// Collects the string arguments naming classes that are not PascalCase.
struct Finder<'a> {
    source: &'a Source,
    functions: Vec<String>,
    found: Vec<std::ops::Range<usize>>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let Some((name, arguments)) = expression.as_call(self.source) else {
            return;
        };
        if !self.functions.contains(&name) {
            return;
        }
        let Some(argument) = arguments.values.first() else {
            return;
        };
        if let Some(contents) = argument.string_contents(self.source)
            && !is_pascal_path(contents)
        {
            self.found.push(argument.range.clone());
        }
    }
}

/// Whether every `::` or `.` separated segment of `path` is PascalCase.
fn is_pascal_path(path: &str) -> bool {
    path.split("::")
        .flat_map(|part| part.split('.'))
        .all(|segment| {
            segment.starts_with(|c: char| c.is_ascii_uppercase())
                && segment.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

inventory::submit! { Registration(|| Box::new(ClassName)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Naming/ClassName";
    const MESSAGE: &str = "Use PascalCase for class and module names.";

    #[test]
    fn accepts_pascal_case_names() {
        expect_no_offenses(
            READER,
            "class 'DateTime' extends 'Date'\nmod 'Lumen::Compiler'\nmod 'MVC'\n\
             class('ActiveRecord::Base')\nmod 'Flux.Undo2'\n",
        );
    }

    #[test]
    fn flags_other_names() {
        expect_offense(READER, "class 'date_time'\n", 1, 7, MESSAGE);
        expect_offense(READER, "mod 'Lumen::styleSheet'\n", 1, 5, MESSAGE);
        expect_offense(
            READER,
            "class 'Thing' extends 'base_thing'\n",
            1,
            23,
            MESSAGE,
        );
        expect_offense(READER, "mod('Item_Base')\n", 1, 5, MESSAGE);
    }

    #[test]
    fn ignores_other_calls_and_dynamic_names() {
        expect_no_offenses(
            READER,
            "print('lower_case')\nclass(name)\nlocal t = { class = 'x' }\n",
        );
    }

    #[test]
    fn functions_are_configurable() {
        let offenses = inspect_with(READER, "library 'my_lib'\n", "Functions: [library]");
        assert_eq!(offenses.len(), 1);
        assert!(inspect(READER, "library 'my_lib'\n").is_empty());
    }
}
