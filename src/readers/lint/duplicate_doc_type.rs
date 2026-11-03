//! `Lint/DuplicateDocType`.

use std::collections::HashMap;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::doc_annotations::{
    TypeDeclaration, annotations, is_meta_file, type_declaration,
};

/// Flags an EmmyLua type (`---@class`, `---@interface`, `---@enum` or `---@alias`) defined again
/// in the same file.
///
/// A class or enum declared again without `(partial)` after a declaration without it is a
/// duplicate; a `(partial)` declaration extends the class or enum and is accepted. An alias, or a
/// name declared with two different tags, is always a duplicate. `---@meta` files are skipped.
///
/// Disabled by default, like the language server's `duplicate-type`: GLuaLS treats every class
/// as partial, so declaring a class in several places is how its fields are spread out.
pub struct DuplicateDocType;

impl Reader for DuplicateDocType {
    fn name(&self) -> &'static str {
        "Lint/DuplicateDocType"
    }

    fn description(&self) -> &'static str {
        "Checks for EmmyLua classes, enums and aliases defined twice in a file."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let found = annotations(ctx.source);
        if is_meta_file(&found) {
            return;
        }
        let mut seen: HashMap<&str, Vec<TypeDeclaration>> = HashMap::new();
        for declaration in found.iter().filter_map(type_declaration) {
            let earlier = seen.entry(declaration.name).or_default();
            if let Some(message) = conflict(earlier, &declaration) {
                ctx.add_offense(declaration.name_range.clone(), message);
            }
            earlier.push(declaration);
        }
    }
}

/// The tag family of `tag`: an interface is a class.
fn family(tag: &str) -> &str {
    if tag == "interface" { "class" } else { tag }
}

/// The message for `declaration` when it conflicts with one of the `earlier` declarations of
/// its name.
fn conflict(earlier: &[TypeDeclaration], declaration: &TypeDeclaration) -> Option<String> {
    let name = declaration.name;
    let noun = |tag: &str| match tag {
        "class" => "Class",
        "interface" => "Interface",
        "enum" => "Enum",
        _ => "Alias",
    };
    if let Some(other) = earlier
        .iter()
        .find(|other| family(other.tag) != family(declaration.tag))
    {
        return Some(format!(
            "Type `{name}` is already defined as {} on line {}.",
            noun(other.tag).to_lowercase(),
            other.line
        ));
    }
    if declaration.tag == "alias" {
        let first = earlier.first()?;
        return Some(format!(
            "Alias `{name}` is already defined on line {}.",
            first.line
        ));
    }
    if declaration.partial {
        return None;
    }
    let first = earlier.iter().find(|other| !other.partial)?;
    Some(format!(
        "{} `{name}` is already defined on line {}; mark the later definition `(partial)` if it extends it.",
        noun(declaration.tag),
        first.line
    ))
}

inventory::submit! { Registration(|| Box::new(DuplicateDocType)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/DuplicateDocType";

    #[test]
    fn flags_types_defined_twice() {
        expect_offenses(
            READER,
            "---@class A\n---@class (partial) A\n---@class A : B\n---@alias Id integer\n\
             ---@alias Id string\n---@enum Kind\n---@class Kind\n---@enum (partial) E\n---@enum E\n",
            &[
                (
                    3,
                    11,
                    "Class `A` is already defined on line 1; mark the later definition `(partial)` if it extends it.",
                ),
                (5, 11, "Alias `Id` is already defined on line 4."),
                (7, 11, "Type `Kind` is already defined as enum on line 6."),
            ],
        );
    }

    #[test]
    fn accepts_partial_classes_distinct_names_and_meta_files() {
        expect_no_offenses(
            READER,
            "---@class A\n---@class (partial) A\n---@class (partial) A\n---@enum (partial) E\n\
             ---@enum (partial) E\n---@class B\n---@alias C B\n-- @alias [D]\n-- @alias [D]\n",
        );
        expect_no_offenses(READER, "---@meta\n---@class A\n---@class A\n");
    }

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }
}
