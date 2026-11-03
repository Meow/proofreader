//! `Lint/DocSyntax`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::doc_annotations::{Annotation, Cursor, Problem, annotations};
use crate::source::Source;

/// Flags EmmyLua annotations (`---@tag`, see [`doc_annotations`](super::doc_annotations)) that
/// a language server cannot read:
///
/// - a missing name after `@class`, `@interface`, `@enum`, `@alias`, `@field`, `@param` or
///   `@generic`;
/// - a missing type after `@type`, after the name of an `@field`, after the name of an `@alias`
///   (unless `---|` lines follow) or after the `:` of a class header;
/// - a bracket (`(`, `[`, `{`, `<` after a name) or quote opened in a type and never closed, or a
///   closing bracket that closes nothing.
///
/// Only the type expressions are checked, never the description after them, so prose such as
/// `--- @param name the player's name (optional` is accepted. A missing type after `@param` and
/// `@return` is accepted too, as LDoc writes those tags with a description only. Comments with
/// two dashes (`-- @field name description`) are not annotations and are never checked.
pub struct DocSyntax;

impl Reader for DocSyntax {
    fn name(&self) -> &'static str {
        "Lint/DocSyntax"
    }

    fn description(&self) -> &'static str {
        "Checks for EmmyLua annotations with a missing name or type or an unclosed bracket."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for annotation in annotations(source) {
            if let Err(problem) = check(source, &annotation) {
                ctx.add_offense(problem.range, problem.message);
            }
        }
    }
}

/// Checks one annotation.
fn check(source: &Source, annotation: &Annotation) -> Result<(), Problem> {
    let mut cursor = Cursor::new(annotation);
    let missing = |what: &str| Problem {
        range: annotation.range.clone(),
        message: format!("Missing {what} after `@{}`.", annotation.tag),
    };
    match annotation.tag {
        "class" | "interface" => {
            cursor.skip_spaces();
            cursor.flags()?;
            cursor.skip_spaces();
            cursor.name().ok_or_else(|| missing("name"))?;
            cursor.generic_list()?;
            cursor.skip_spaces();
            if cursor.peek() == Some(':') {
                let colon = cursor.offset();
                cursor.eat(':');
                if cursor.type_expression(true)?.is_none() {
                    return Err(Problem {
                        range: colon..colon + 1,
                        message: "Missing parent type after `:`.".to_owned(),
                    });
                }
            }
        }
        "enum" => {
            cursor.skip_spaces();
            cursor.flags()?;
            cursor.skip_spaces();
            cursor.name().ok_or_else(|| missing("name"))?;
            cursor.skip_spaces();
            if cursor.eat(':') && cursor.type_expression(false)?.is_none() {
                return Err(missing("type"));
            }
        }
        "alias" => {
            cursor.skip_spaces();
            cursor.flags()?;
            cursor.skip_spaces();
            let (name, range) = cursor.name().ok_or_else(|| missing("name"))?;
            cursor.generic_list()?;
            let continued = source
                .line(annotation.line + 1)
                .trim_start()
                .starts_with("---|");
            if cursor.type_expression(false)?.is_none() && !continued {
                return Err(Problem {
                    range,
                    message: format!("Missing type of alias `{name}`."),
                });
            }
        }
        "field" => {
            cursor.skip_spaces();
            cursor.flags()?;
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
                cursor.type_expression(false)?;
                start..cursor.offset()
            } else {
                cursor.name().ok_or_else(|| missing("name"))?.1
            };
            cursor.eat('?');
            if cursor.type_expression(false)?.is_none() {
                let name = &source.text[range.clone()];
                return Err(Problem {
                    range,
                    message: format!("Missing type of field `{name}`."),
                });
            }
        }
        "param" => {
            skip_ldoc_modifier(&mut cursor)?;
            cursor.skip_spaces();
            if !cursor.rest().starts_with("...") {
                cursor.name().ok_or_else(|| missing("parameter name"))?;
            }
            cursor.eat('?');
            check_type(&mut cursor, false)?;
        }
        "return" => {
            skip_ldoc_modifier(&mut cursor)?;
            check_type(&mut cursor, true)?;
        }
        "type" => {
            if cursor.type_expression(true)?.is_none() {
                return Err(missing("type"));
            }
        }
        "generic" => loop {
            cursor.skip_spaces();
            cursor.name().ok_or_else(|| missing("name"))?;
            cursor.skip_spaces();
            if cursor.eat(':') {
                cursor.type_expression(false)?;
                cursor.skip_spaces();
            }
            if !cursor.eat(',') {
                break;
            }
        },
        "overload" | "as" => check_type(&mut cursor, false)?,
        _ => {}
    }
    Ok(())
}

/// Checks the brackets of the type at the cursor, if there is one.
fn check_type(cursor: &mut Cursor, list: bool) -> Result<(), Problem> {
    cursor.type_expression(list).map(|_| ())
}

/// Skips an LDoc modifier written right after the tag (`@param[opt] name`, `@return[1]`).
fn skip_ldoc_modifier(cursor: &mut Cursor) -> Result<(), Problem> {
    if cursor.peek() == Some('[') {
        cursor.type_expression(false)?;
    }
    Ok(())
}

inventory::submit! { Registration(|| Box::new(DocSyntax)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/DocSyntax";

    #[test]
    fn flags_missing_names() {
        expect_offenses(
            READER,
            "---@class\n---@alias  \n---@enum (partial)\n---@field\n---@param\n\
             --- @generic\n---@class (exact)\n",
            &[
                (1, 4, "Missing name after `@class`."),
                (2, 4, "Missing name after `@alias`."),
                (3, 4, "Missing name after `@enum`."),
                (4, 4, "Missing name after `@field`."),
                (5, 4, "Missing parameter name after `@param`."),
                (6, 5, "Missing name after `@generic`."),
                (7, 4, "Missing name after `@class`."),
            ],
        );
    }

    #[test]
    fn flags_missing_types() {
        expect_offenses(
            READER,
            "---@type\n---@field name\n---@field private count? @desc\n---@alias Id\n\
             ---@class A :\n---@type # just a description\n",
            &[
                (1, 4, "Missing type after `@type`."),
                (2, 11, "Missing type of field `name`."),
                (3, 19, "Missing type of field `count`."),
                (4, 11, "Missing type of alias `Id`."),
                (5, 13, "Missing parent type after `:`."),
                (6, 4, "Missing type after `@type`."),
            ],
        );
    }

    #[test]
    fn flags_unclosed_brackets() {
        expect_offenses(
            READER,
            "---@type table<string, number\n---@param cb fun(a: number desc\n\
             ---@field x { y: number\n---@return string)\n---@class A<T : B\n\
             ---@field [string number\n---@alias Mode 'a' | 'b\n---@class (exact A\n",
            &[
                (1, 15, "`<` is never closed."),
                (2, 17, "`(` is never closed."),
                (3, 13, "`{` is never closed."),
                (4, 18, "`)` closes no bracket."),
                (5, 12, "`<` is never closed."),
                (6, 11, "`[` is never closed."),
                (7, 22, "`'` is never closed."),
                (8, 11, "`(` is never closed."),
            ],
        );
    }

    #[test]
    fn accepts_well_formed_annotations() {
        expect_no_offenses(
            READER,
            "---@class (partial) A<T> : B, C<T>\n---@field public name string\n\
             ---@field [string] number\n---@field ['k'] boolean\n---@field private x? integer\n\
             ---@field cb fun(a: number, ...: any): string, boolean\n---@type number, string\n\
             ---@type { [string]: IMesh }\n---@type table # current batching table\n\
             ---@param ... any\n---@param a? number|nil\n---@return number|nil totalCm\n\
             ---@return (instance) DPanel\n---@generic T, K: table\n---@enum Kind : integer\n\
             ---@alias Mode\n---| 'a'\n---| 'b'\n---@alias Id integer\n\
             ---@overload fun(a: number): string\n---@type string[]?\n--- @field Name string @The name\n\
             --- @field MinAccess \"'user'\" | \"'admin'\" @Default group\n\
             ---@param target GPlayer | nil @Optional - the target\n\
             ---@alias ffi.ct*     ffi.ctype*|ffi.cdecl*\n---@class `T`.bbb\n---@class aaa.`T`\n",
        );
    }

    #[test]
    fn accepts_ldoc_and_descriptions() {
        expect_no_offenses(
            READER,
            "-- @field string A regular string (one word\n-- @alias [Package.present]\n\
             -- @param\n--- @param name the player's name (optional\n--- @return\n\
             --- @param[opt] x desc\n---@return[1] boolean ok\n---- @class\n\
             --[[ @class ]]\n---@realm server\n---@diagnostic disable-next-line: undefined-global\n",
        );
    }
}
