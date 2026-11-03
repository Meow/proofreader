//! EmmyLua annotations (`---@class Name`), shared by the readers that check them.
//!
//! An annotation is a `--` line comment whose text starts with exactly three dashes, optionally
//! followed by spaces or tabs, and then an `@tag` (`---@class A`, `--- @field x number`).
//! Comments with two dashes (LDoc's `-- @field name description`, Flux's `-- @alias [X]`) or with
//! four or more are not annotations, so LDoc and Flux documentation is never read as EmmyLua.
//!
//! The helpers read just enough of the annotation grammar to find names and the extent of type
//! expressions. A type expression is a run of atoms, each a stretch of text without spaces
//! outside brackets and strings (`table<string, number>`, `fun(a: number):`, `"'user'"`), joined
//! by `|`, `&` or, where a list is allowed, `,`, or by an atom ending in one of them or in `:`
//! (`fun(): string`). Whatever follows is description, which is never checked.

use std::ops::Range;

use crate::readers::style::doc_comments::{comment_line, tag};
use crate::source::Source;
use crate::token::TokenKind;

/// An `@tag` annotation in a `---` line comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation<'a> {
    /// The tag name without `@` (`class`).
    pub tag: &'a str,
    /// Byte range of the tag including `@`.
    pub range: Range<usize>,
    /// The text after the tag name, trailing whitespace removed.
    pub rest: &'a str,
    /// Byte offset of `rest`.
    pub rest_start: usize,
    /// The line of the comment.
    pub line: u32,
    /// Whether the comment is the only thing on its line.
    pub alone: bool,
}

/// The annotations of `source`, in source order.
pub fn annotations(source: &Source) -> Vec<Annotation<'_>> {
    source
        .code_tokens()
        .filter(|token| token.kind == (TokenKind::Comment { long: false }))
        .filter_map(|comment| {
            let text = source.text_of(comment);
            if !text.starts_with("---") || text.starts_with("----") {
                return None;
            }
            let tag = tag(source, comment)?;
            Some(Annotation {
                tag: tag.name,
                range: tag.range,
                rest: tag.rest.trim_end(),
                rest_start: tag.rest_start,
                line: comment.line,
                alone: comment_line(source, comment.line) == Some(comment),
            })
        })
        .collect()
}

/// Whether the file is a `---@meta` definition file, whose declarations describe code defined
/// elsewhere.
pub fn is_meta_file(annotations: &[Annotation]) -> bool {
    annotations
        .iter()
        .any(|annotation| annotation.tag == "meta")
}

/// Whether lines `from..to` (exclusive) all hold nothing but a `--` line comment, so that the
/// annotations around them belong to one doc comment.
pub fn comment_lines_between(source: &Source, from: u32, to: u32) -> bool {
    (from + 1..to).all(|line| comment_line(source, line).is_some())
}

/// A malformed part of an annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The bytes to report.
    pub range: Range<usize>,
    /// What is wrong, as a sentence.
    pub message: String,
}

/// A name with its byte range.
pub type Name<'a> = (&'a str, Range<usize>);

/// A position in the text of an annotation.
#[derive(Debug, Clone, Copy)]
pub struct Cursor<'a> {
    text: &'a str,
    pos: usize,
    base: usize,
}

/// Whether `c` can start a doc name.
fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// Whether `c` can continue a doc name.
fn is_name_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of the text after the tag of `annotation`.
    pub fn new(annotation: &Annotation<'a>) -> Self {
        Cursor {
            text: annotation.rest,
            pos: 0,
            base: annotation.rest_start,
        }
    }

    /// The byte offset of the cursor in the source.
    pub fn offset(&self) -> usize {
        self.base + self.pos
    }

    /// The text from the cursor on.
    pub fn rest(&self) -> &'a str {
        &self.text[self.pos..]
    }

    /// The next character.
    pub fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// The character before the cursor.
    fn previous(&self) -> Option<char> {
        self.text[..self.pos].chars().next_back()
    }

    /// Moves past the next character.
    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.pos += c.len_utf8();
        }
    }

    /// Moves past `c` when it is the next character.
    pub fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.bump();
        }
        found
    }

    /// Moves past spaces and tabs; returns whether there were any.
    pub fn skip_spaces(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
        self.pos > start
    }

    /// Reads a doc name (`Name`, `a.b`, `x-y`, `ffi.ct*`) as the language server does: a letter
    /// or `_`, then letters, digits, `_`, backticks and `.`, `-` or `*` not followed by another
    /// of those three.
    pub fn name(&mut self) -> Option<Name<'a>> {
        let start = self.pos;
        if !self.peek().is_some_and(|c| is_name_start(c) || c == '`') {
            return None;
        }
        self.bump();
        loop {
            match self.peek() {
                Some(c) if is_name_continue(c) || c == '`' => self.bump(),
                Some('.' | '-' | '*') => {
                    let mut after = self.rest().chars().skip(1);
                    if after.next().is_some_and(|c| matches!(c, '.' | '-' | '*')) {
                        break;
                    }
                    self.bump();
                }
                _ => break,
            }
        }
        Some((
            &self.text[start..self.pos],
            self.base + start..self.base + self.pos,
        ))
    }

    /// Reads a parenthesized list of flags (`(partial)`, `(exact, private)`) when the cursor
    /// is at `(`; returns the flag names.
    pub fn flags(&mut self) -> Result<Vec<&'a str>, Problem> {
        if self.peek() != Some('(') {
            return Ok(Vec::new());
        }
        let open = self.offset();
        self.bump();
        let mut flags = Vec::new();
        loop {
            self.skip_spaces();
            if let Some((name, _)) = self.name() {
                flags.push(name);
            }
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.bump(),
                Some(')') => {
                    self.bump();
                    return Ok(flags);
                }
                _ => return Err(unclosed('(', open)),
            }
        }
    }

    /// Reads a balanced `<...>` list of generic parameters or arguments when the cursor is at
    /// `<`.
    pub fn generic_list(&mut self) -> Result<(), Problem> {
        if self.peek() == Some('<') {
            self.atom()?;
        }
        Ok(())
    }

    /// Reads a type expression and returns its byte range, or `None` (without moving) when the
    /// text at the cursor cannot start one. With `list`, types separated by `,` are one list.
    pub fn type_expression(&mut self, list: bool) -> Result<Option<Range<usize>>, Problem> {
        let mut probe = *self;
        probe.skip_spaces();
        if !probe.peek().is_some_and(starts_type) {
            return Ok(None);
        }
        *self = probe;
        let start = self.offset();
        loop {
            self.atom()?;
            let end = self.offset();
            let ended_with_joint = self.text[..self.pos]
                .chars()
                .next_back()
                .is_some_and(|c| is_joint(c, list) || c == ':');
            let mut next = *self;
            next.skip_spaces();
            let joined = ended_with_joint || next.peek().is_some_and(|c| is_joint(c, list));
            if !joined {
                return Ok(Some(start..end));
            }
            if !ended_with_joint {
                next.bump();
                next.skip_spaces();
            }
            if !next.peek().is_some_and(starts_type) {
                *self = next;
                return Ok(Some(start..end));
            }
            *self = next;
        }
    }

    /// Reads one atom of a type: text without spaces outside brackets and strings, with every
    /// bracket it opens closed. A quote starts a string only where a name cannot continue
    /// (`"'user'"`, but not the apostrophe in `Player's`); `<` opens a bracket only after a name
    /// (`table<K, V>`).
    fn atom(&mut self) -> Result<(), Problem> {
        let mut open: Vec<(char, usize)> = Vec::new();
        while let Some(c) = self.peek() {
            if open.is_empty() && (c == ' ' || c == '\t') {
                break;
            }
            let at = self.offset();
            match c {
                '"' | '\'' | '`'
                    if !self
                        .previous()
                        .is_some_and(|p| is_name_continue(p) || p == '`') =>
                {
                    self.bump();
                    loop {
                        match self.peek() {
                            None => return Err(unclosed(c, at)),
                            Some('\\') => {
                                self.bump();
                                self.bump();
                            }
                            Some(q) if q == c => {
                                self.bump();
                                break;
                            }
                            Some(_) => self.bump(),
                        }
                    }
                    continue;
                }
                '(' | '[' | '{' => open.push((c, at)),
                '<' if self
                    .previous()
                    .is_some_and(|p| is_name_continue(p) || p == '>') =>
                {
                    open.push((c, at));
                }
                '>' if open.last().is_some_and(|(o, _)| *o == '<') => {
                    open.pop();
                }
                ')' | ']' | '}' => {
                    let expected = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    match open.last() {
                        Some((o, _)) if *o == expected => {
                            open.pop();
                        }
                        Some((o, position)) => return Err(unclosed(*o, *position)),
                        None => {
                            return Err(Problem {
                                range: at..at + 1,
                                message: format!("`{c}` closes no bracket."),
                            });
                        }
                    }
                }
                _ => {}
            }
            self.bump();
        }
        match open.last() {
            Some((o, position)) => Err(unclosed(*o, *position)),
            None => Ok(()),
        }
    }
}

/// Whether `c` joins two types: `|`, `&` and, in a list, `,`.
fn is_joint(c: char, list: bool) -> bool {
    c == '|' || c == '&' || (list && c == ',')
}

/// Whether a type can start with `c`: a name, a number, a string, a bracket or `...`.
fn starts_type(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || matches!(c, '"' | '\'' | '`' | '(' | '{' | '[' | '.')
}

/// The problem of an `open` bracket or quote at `at` that is never closed.
fn unclosed(open: char, at: usize) -> Problem {
    Problem {
        range: at..at + 1,
        message: format!("`{open}` is never closed."),
    }
}

/// A type declared by `@class`, `@interface`, `@enum` or `@alias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration<'a> {
    /// The tag name (`class`).
    pub tag: &'a str,
    /// The declared name.
    pub name: &'a str,
    /// Byte range of the name.
    pub name_range: Range<usize>,
    /// Whether the declaration is marked `(partial)`.
    pub partial: bool,
    /// The parent types of a class that are plain names (`---@class A : B, C`), with their
    /// ranges.
    pub parents: Vec<Name<'a>>,
    /// The line of the annotation.
    pub line: u32,
}

/// The tags that declare a type.
pub const TYPE_TAGS: [&str; 4] = ["class", "interface", "enum", "alias"];

/// The type `annotation` declares, when it is a well-formed enough `@class`, `@interface`,
/// `@enum` or `@alias`.
pub fn type_declaration<'a>(annotation: &Annotation<'a>) -> Option<TypeDeclaration<'a>> {
    if !TYPE_TAGS.contains(&annotation.tag) {
        return None;
    }
    let mut cursor = Cursor::new(annotation);
    cursor.skip_spaces();
    let flags = cursor.flags().ok()?;
    cursor.skip_spaces();
    let (name, name_range) = cursor.name()?;
    let mut parents = Vec::new();
    if matches!(annotation.tag, "class" | "interface") {
        cursor.generic_list().ok()?;
        cursor.skip_spaces();
        if cursor.eat(':') {
            loop {
                cursor.skip_spaces();
                let Some(parent) = cursor.name() else {
                    break;
                };
                if !cursor
                    .peek()
                    .is_some_and(|c| !matches!(c, ',' | ' ' | '\t'))
                {
                    parents.push(parent);
                }
                while cursor.peek().is_some_and(|c| c != ',') {
                    cursor.bump();
                }
                if !cursor.eat(',') {
                    break;
                }
            }
        }
    }
    Some(TypeDeclaration {
        tag: annotation.tag,
        name,
        name_range,
        partial: flags.contains(&"partial"),
        parents,
        line: annotation.line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(text: &str) -> (Source, usize) {
        let source = Source::new("t.lua", text);
        let count = annotations(&source).len();
        (source, count)
    }

    #[test]
    fn only_triple_dash_comments_are_annotations() {
        let (source, count) = first(
            "---@class A\n--- @field x number\n-- @field y desc\n---- @class B\n--[[@as x]]\n\
             local a = 1 ---@type number\n",
        );
        assert_eq!(count, 3);
        let found = annotations(&source);
        assert_eq!(found[0].tag, "class");
        assert_eq!(found[0].rest, " A");
        assert!(found[0].alone);
        assert!(!found[2].alone);
    }

    fn type_of(rest: &str, list: bool) -> Result<Option<String>, String> {
        let text = format!("---@type{rest}\n");
        let source = Source::new("t.lua", text.clone());
        let annotation = annotations(&source).remove(0);
        let mut cursor = Cursor::new(&annotation);
        cursor
            .type_expression(list)
            .map(|range| range.map(|range| text[range].to_owned()))
            .map_err(|problem| problem.message)
    }

    #[test]
    fn reads_type_expressions() {
        assert_eq!(type_of(" string desc", false), Ok(Some("string".into())));
        assert_eq!(
            type_of(" string | nil @desc", false),
            Ok(Some("string | nil".into()))
        );
        assert_eq!(
            type_of(" \"'user'\" | \"'admin'\" @x", false),
            Ok(Some("\"'user'\" | \"'admin'\"".into()))
        );
        assert_eq!(
            type_of(" fun(a: number): string the rest", false),
            Ok(Some("fun(a: number): string".into()))
        );
        assert_eq!(
            type_of(" table<string, number>[] x", false),
            Ok(Some("table<string, number>[]".into()))
        );
        assert_eq!(
            type_of(" number, string desc", true),
            Ok(Some("number, string".into()))
        );
        assert_eq!(
            type_of(" number, string desc", false),
            Ok(Some("number,".into()))
        );
        assert_eq!(type_of(" @desc", false), Ok(None));
        assert_eq!(type_of("", false), Ok(None));
        assert_eq!(
            type_of(" Player's name (optional", false),
            Ok(Some("Player's".into()))
        );
    }

    #[test]
    fn reports_unbalanced_brackets() {
        assert_eq!(
            type_of(" table<string, number", false),
            Err("`<` is never closed.".into())
        );
        assert_eq!(
            type_of(" fun(a: number", false),
            Err("`(` is never closed.".into())
        );
        assert_eq!(
            type_of(" {x: (number}", false),
            Err("`(` is never closed.".into())
        );
        assert_eq!(
            type_of(" number)", false),
            Err("`)` closes no bracket.".into())
        );
        assert_eq!(
            type_of(" \"open", false),
            Err("`\"` is never closed.".into())
        );
    }

    #[test]
    fn reads_type_declarations() {
        let source = Source::new(
            "t.lua",
            "---@class (partial) A<T> : B, C<T>, D\n---@alias X string\n---@field y z\n",
        );
        let found = annotations(&source);
        let class = type_declaration(&found[0]).expect("class");
        assert_eq!(class.name, "A");
        assert!(class.partial);
        let parents: Vec<&str> = class.parents.iter().map(|(name, _)| *name).collect();
        assert_eq!(parents, ["B", "D"]);
        assert_eq!(type_declaration(&found[1]).expect("alias").name, "X");
        assert!(type_declaration(&found[2]).is_none());
    }
}
