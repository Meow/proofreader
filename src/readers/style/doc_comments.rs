//! Doc comments of function definitions, shared by the documentation readers.
//!
//! The doc comment of a function is the run of comment-only `--` lines directly above the line
//! of its `function` keyword; only the first `function` on a line owns the comment above it.
//! This covers `function a.b()`, `local function f()`, `local f = function()`,
//! `x.y = function()` and callbacks such as `hook.Add('A', 'b', function(p) ... end)`.
//!
//! A tag is an `@name` at the start of a comment's text: `-- @param name [Type desc]` (Flux),
//! `---@param name type desc` (EmmyLua) and `-- @tparam type name desc` (LDoc) all have one.

use std::ops::Range;

use crate::readers::lint::ast::{self, Block, Function, StatementKind, Visitor};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// LDoc's built-in aliases of `@tparam`: `@string name desc` documents the parameter `name`.
pub const LDOC_TYPED_PARAM_TAGS: [&str; 7] =
    ["string", "number", "int", "bool", "tab", "func", "thread"];

/// A tag in a line comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag<'a> {
    /// The tag name without `@` (`param`).
    pub name: &'a str,
    /// Byte range of the tag including `@`.
    pub range: Range<usize>,
    /// The text after the tag name.
    pub rest: &'a str,
    /// Byte offset of `rest`.
    pub rest_start: usize,
}

/// The parameter documented by a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamTag<'a> {
    /// The parameter name as written, without a default (`=1`) or optional marker (`?`).
    pub name: &'a str,
    /// Byte range of the name.
    pub range: Range<usize>,
}

/// The doc comment of a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocComment<'a> {
    /// The comment tokens, top to bottom.
    pub comments: Vec<&'a Token>,
    /// The tags of those comments, top to bottom.
    pub tags: Vec<Tag<'a>>,
}

impl<'a> DocComment<'a> {
    /// The parameters documented by `@param name`, `@tparam type name` and the `typed` aliases
    /// of `@tparam` (`@string name`).
    pub fn params<S: AsRef<str>>(&self, typed: &[S]) -> Vec<ParamTag<'a>> {
        self.tags
            .iter()
            .filter_map(|tag| {
                let skip = match tag.name {
                    "param" => 0,
                    "tparam" => 1,
                    name if typed.iter().any(|alias| alias.as_ref() == name) => 0,
                    _ => return None,
                };
                param_name(tag.rest, tag.rest_start, skip)
            })
            .collect()
    }

    /// Whether the comment has a parameter tag (see [`DocComment::params`]) or a return tag
    /// (`@return`, `@treturn`).
    pub fn has_signature_tags<S: AsRef<str>>(&self, typed: &[S]) -> bool {
        self.has_return() || !self.params(typed).is_empty()
    }

    /// Whether the comment has an `@return` or `@treturn` tag.
    pub fn has_return(&self) -> bool {
        self.tags
            .iter()
            .any(|tag| matches!(tag.name, "return" | "treturn"))
    }
}

/// The comment token of line `n` when the line holds nothing but a `--` line comment.
pub fn comment_line(source: &Source, n: u32) -> Option<&Token> {
    let first = source.first_code_token_on_line(n)?;
    let last = source.last_code_token_on_line(n)?;
    (first == last
        && first.kind == (TokenKind::Comment { long: false })
        && source.text_of(first).starts_with("--"))
    .then_some(first)
}

/// The tag of the line comment `comment`, if its text starts with one.
pub fn tag<'a>(source: &'a Source, comment: &Token) -> Option<Tag<'a>> {
    if comment.kind != (TokenKind::Comment { long: false }) {
        return None;
    }
    let text = source.text_of(comment);
    let body = text.strip_prefix("--")?;
    let body = body.strip_prefix('-').unwrap_or(body);
    let trimmed = body.trim_start_matches([' ', '\t']);
    let after_at = trimmed.strip_prefix('@')?;
    let name_len = after_at
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after_at.len());
    if name_len == 0 {
        return None;
    }
    let at = comment.start + text.len() - trimmed.len();
    let name_end = at + 1 + name_len;
    Some(Tag {
        name: &after_at[..name_len],
        range: at..name_end,
        rest: &after_at[name_len..],
        rest_start: name_end,
    })
}

/// The parameter name in `rest` (starting at byte `start`) after skipping `skip` words.
fn param_name(rest: &str, start: usize, skip: usize) -> Option<ParamTag<'_>> {
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let leading = |offset: usize| rest[offset..].len() - rest[offset..].trim_start().len();
    let mut offset = 0;
    for _ in 0..skip {
        offset += leading(offset);
        offset += rest[offset..]
            .find(char::is_whitespace)
            .unwrap_or(rest.len() - offset);
    }
    offset += leading(offset);
    let word = &rest[offset..];
    let len = if word.starts_with("...") {
        3
    } else {
        word.find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(word.len())
    };
    (len > 0).then(|| ParamTag {
        name: &word[..len],
        range: start + offset..start + offset + len,
    })
}

/// The doc comment of the function whose `function` keyword is `keyword`: the comment-only
/// `--` lines directly above its line, when it is the first `function` on that line.
pub fn doc_comment<'a>(source: &'a Source, keyword: &Token) -> Option<DocComment<'a>> {
    let first_function = source
        .code_tokens_on_line(keyword.line)
        .find(|token| token.kind == TokenKind::Function);
    if first_function != Some(keyword) {
        return None;
    }
    let mut comments: Vec<&Token> = (1..keyword.line)
        .rev()
        .map_while(|line| comment_line(source, line))
        .collect();
    if comments.is_empty() {
        return None;
    }
    comments.reverse();
    let tags = comments
        .iter()
        .filter_map(|comment| tag(source, comment))
        .collect();
    Some(DocComment { comments, tags })
}

/// A function of a file with what the documentation readers need to know about it.
#[derive(Debug, Clone)]
pub struct DocumentedFunction<'t, 'a> {
    /// The function.
    pub function: &'t Function,
    /// The name it is defined or assigned under (`a.b:c`, `f`), if any.
    pub name: Option<String>,
    /// Whether it is a `function a:b()` method with an implicit `self`.
    pub method: bool,
    /// Its doc comment, if any.
    pub doc: Option<DocComment<'a>>,
}

impl DocumentedFunction<'_, '_> {
    /// How messages refer to the function: `` `a.b:c` `` or `this function`.
    pub fn describe(&self) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None => "this function".to_owned(),
        }
    }

    /// Whether the function has a parameter called `name` (`self` for methods, `...` for a
    /// vararg function).
    pub fn has_parameter(&self, source: &Source, name: &str) -> bool {
        match name {
            "..." => self.function.vararg.is_some(),
            "self" if self.method => true,
            _ => self
                .function
                .parameters
                .iter()
                .any(|parameter| source.text_of(parameter) == name),
        }
    }
}

/// Every function of `block`, in source order, with its name and doc comment.
pub fn functions<'t, 'a>(source: &'a Source, block: &'t Block) -> Vec<DocumentedFunction<'t, 'a>> {
    struct Collector<'t, 'a> {
        source: &'a Source,
        names: Vec<(usize, String, bool)>,
        found: Vec<DocumentedFunction<'t, 'a>>,
    }
    impl<'t> Visitor<'t> for Collector<'t, '_> {
        fn statement(&mut self, statement: &'t ast::Statement) {
            let source = self.source;
            match &statement.kind {
                StatementKind::Function { name, function } => {
                    let text: Vec<&str> = name
                        .path
                        .iter()
                        .map(|token| source.text_of(token))
                        .collect();
                    let mut full = text.join(".");
                    if let Some(method) = &name.method {
                        full.push(':');
                        full.push_str(source.text_of(method));
                    }
                    self.names
                        .push((function.keyword.start, full, name.method.is_some()));
                }
                StatementKind::LocalFunction { name, function, .. } => {
                    self.names.push((
                        function.keyword.start,
                        source.text_of(name).to_owned(),
                        false,
                    ));
                }
                StatementKind::Local { names, values, .. } => {
                    for (name, value) in names.iter().zip(values) {
                        if let ast::ExpressionKind::Function(function) = &value.kind {
                            self.names.push((
                                function.keyword.start,
                                source.text_of(name).to_owned(),
                                false,
                            ));
                        }
                    }
                }
                StatementKind::Assignment {
                    targets, values, ..
                } => {
                    for (target, value) in targets.iter().zip(values) {
                        if let (ast::ExpressionKind::Function(function), Some(name)) =
                            (&value.kind, target.dotted_name(source))
                        {
                            self.names.push((function.keyword.start, name, false));
                        }
                    }
                }
                _ => {}
            }
        }

        fn enter_function(&mut self, function: &'t Function) {
            let (name, method) = self
                .names
                .iter()
                .rev()
                .find(|(start, _, _)| *start == function.keyword.start)
                .map_or((None, false), |(_, name, method)| {
                    (Some(name.clone()), *method)
                });
            self.found.push(DocumentedFunction {
                function,
                name,
                method,
                doc: doc_comment(self.source, &function.keyword),
            });
        }
    }
    let mut collector = Collector {
        source,
        names: Vec::new(),
        found: Vec::new(),
    };
    ast::walk(block, &mut collector);
    collector.found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::lint::ast::parse;

    fn params(src: &str) -> Vec<String> {
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let found = functions(&source, &block);
        found[0]
            .doc
            .as_ref()
            .map(|doc| {
                doc.params(&LDOC_TYPED_PARAM_TAGS)
                    .iter()
                    .map(|param| {
                        assert_eq!(&src[param.range.clone()], param.name);
                        param.name.to_owned()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn reads_param_names_in_every_style() {
        assert_eq!(
            params(
                "--- Summary.\n-- @param a [Number]\n-- @param b=nil [Any]\n---@param c? string\n\
                 ---@param ... any\n-- @tparam string d desc\n-- @string e desc\n--   @param f [X]\n\
                 -- @param\n-- @return [Number]\nfunction g() end\n"
            ),
            vec!["a", "b", "c", "...", "d", "e", "f"]
        );
    }

    #[test]
    fn ignores_tags_that_do_not_start_the_comment() {
        assert!(params("-- see @param a\n-- x@param b\nfunction f() end\n").is_empty());
    }

    #[test]
    fn only_the_first_function_on_a_line_owns_the_comment() {
        let src = "-- @param a [Number]\nf(function() end, function(a) end)\n";
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let found = functions(&source, &block);
        assert!(found[0].doc.is_some());
        assert!(found[1].doc.is_none());
    }

    #[test]
    fn blank_lines_and_code_separate_comments() {
        assert!(params("-- @param a\n\nfunction f(a) end\n").is_empty());
        assert!(params("-- @param a\nx = 1\nfunction f(a) end\n").is_empty());
        assert!(params("--[[ @param a ]]\nfunction f(a) end\n").is_empty());
    }

    #[test]
    fn names_functions() {
        let src = "function a.b:c() end\nlocal function d() end\nlocal e = function() end\n\
                   f.g = function() end\nh(function() end)\n";
        let source = Source::new("t.lua", src);
        let block = parse(&source).expect("parses");
        let names: Vec<(Option<String>, bool)> = functions(&source, &block)
            .into_iter()
            .map(|function| (function.name, function.method))
            .collect();
        assert_eq!(
            names,
            vec![
                (Some("a.b:c".to_owned()), true),
                (Some("d".to_owned()), false),
                (Some("e".to_owned()), false),
                (Some("f.g".to_owned()), false),
                (None, false)
            ]
        );
    }

    #[test]
    fn finds_tags() {
        let src = "---@class Foo\n-- @return [X]\n--@realm server\n-- note\nx = 1 -- @todo\n";
        let source = Source::new("t.lua", src);
        let names: Vec<&str> = source
            .code_tokens()
            .filter_map(|token| tag(&source, token))
            .map(|tag| tag.name)
            .collect();
        assert_eq!(names, vec!["class", "return", "realm", "todo"]);
    }
}
