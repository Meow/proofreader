//! `Lint/DuplicateDocField`.

use std::collections::HashMap;
use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::doc_annotations::{
    Annotation, Cursor, annotations, comment_lines_between, type_declaration,
};

/// Flags an `---@field` documenting a field that an earlier `---@field` of the same class in the
/// file already documents.
///
/// A field belongs to the `---@class` (or `---@interface`) above it in the same doc comment: the
/// run of comment-only lines holding both. Declarations of one class in several doc comments
/// (partial classes) share their fields. Fields documented more than once with function types
/// only (`---@field f fun(a: number)` and `---@field f fun(a: string)`) are overloads and
/// accepted, like the language server does. `[key]` fields are compared by their key as written.
pub struct DuplicateDocField;

impl Reader for DuplicateDocField {
    fn name(&self) -> &'static str {
        "Lint/DuplicateDocField"
    }

    fn description(&self) -> &'static str {
        "Checks for fields documented twice for the same class."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let found = annotations(source);
        let mut class: Option<(&str, u32)> = None;
        let mut fields: HashMap<(&str, String), Vec<Field>> = HashMap::new();
        let mut order: Vec<(&str, String)> = Vec::new();
        for annotation in &found {
            if !annotation.alone {
                continue;
            }
            if let Some((_, line)) = class
                && !comment_lines_between(source, line, annotation.line)
            {
                class = None;
            }
            if matches!(annotation.tag, "class" | "interface") {
                class = type_declaration(annotation)
                    .map(|declaration| (declaration.name, annotation.line));
                continue;
            }
            if let Some((name, line)) = class.as_mut() {
                *line = annotation.line;
                if annotation.tag != "field" {
                    continue;
                }
                let Some(field) = field(source.text.as_str(), annotation) else {
                    continue;
                };
                let key = (*name, field.key.clone());
                let entry = fields.entry(key.clone()).or_default();
                if entry.is_empty() {
                    order.push(key);
                }
                entry.push(field);
            }
        }
        for key in order {
            let documented = &fields[&key];
            if documented.len() < 2 || documented.iter().all(|field| field.function) {
                continue;
            }
            let first = &documented[0];
            for field in &documented[1..] {
                ctx.add_offense(
                    field.range.clone(),
                    format!(
                        "Field `{}` of `{}` is already documented on line {}.",
                        field.key, key.0, first.line
                    ),
                );
            }
        }
    }
}

/// A documented field.
struct Field {
    /// The name, or the key with its brackets (`[string]`).
    key: String,
    /// Byte range of the name or key.
    range: Range<usize>,
    /// The line of the annotation.
    line: u32,
    /// Whether the type is a function type (`fun(...)`).
    function: bool,
}

/// The field an `@field` annotation documents, when it is well formed.
fn field(text: &str, annotation: &Annotation) -> Option<Field> {
    let mut cursor = Cursor::new(annotation);
    cursor.skip_spaces();
    cursor.flags().ok()?;
    cursor.skip_spaces();
    let mut probe = cursor;
    if let Some((word, _)) = probe.name()
        && matches!(word, "public" | "private" | "protected" | "package")
        && probe.skip_spaces()
        && probe
            .peek()
            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '[')
    {
        cursor = probe;
    }
    let range = if cursor.peek() == Some('[') {
        let start = cursor.offset();
        cursor.type_expression(false).ok()?;
        start..cursor.offset()
    } else {
        cursor.name()?.1
    };
    cursor.eat('?');
    let ty = cursor.type_expression(false).ok()??;
    let key: String = text[range.clone()]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    Some(Field {
        key,
        range,
        line: annotation.line,
        function: text[ty].starts_with("fun("),
    })
}

inventory::submit! { Registration(|| Box::new(DuplicateDocField)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/DuplicateDocField";

    #[test]
    fn flags_fields_documented_twice() {
        expect_offenses(
            READER,
            "---@class Test\n---@field name string\n---@field age number\n\
             ---@field name number\nlocal Test = {}\n\n---@class (partial) Test\n\
             ---@field public age integer\n---@field [string] any\n---@field [ string ] any\n",
            &[
                (
                    4,
                    11,
                    "Field `name` of `Test` is already documented on line 2.",
                ),
                (
                    8,
                    18,
                    "Field `age` of `Test` is already documented on line 3.",
                ),
                (
                    10,
                    11,
                    "Field `[string]` of `Test` is already documented on line 9.",
                ),
            ],
        );
    }

    #[test]
    fn accepts_other_classes_overloads_and_detached_fields() {
        expect_no_offenses(
            READER,
            "---@class Test1\n---@field name string\n\n---@class Test2\n---@field name string\n\
             ---@field f fun(a: number)\n---@field f fun(a: string)\nlocal t = {}\n\
             ---@field name string\n-- @field name description\n-- @field name description\n",
        );
    }

    #[test]
    fn a_doc_comment_ends_at_the_first_line_that_is_not_a_comment() {
        expect_no_offenses(
            READER,
            "---@class A\n---@field x number\nlocal a = {}\n---@field x number\n",
        );
    }
}
