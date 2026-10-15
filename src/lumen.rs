//! Lumen templates: `.lumen` files are Lua code in which markup may appear wherever an
//! expression may, the format compiled by Flux's `Lumen.Compiler`.
//!
//! ```text
//! return <view style={{ gap = 4 }}>
//!   <text>Hello, {props.name}!</text>
//!   {Lumen.map(props.items, function(item) return <Card key={item.id} /> end)}
//! </view>
//! ```
//!
//! The [lexer](crate::lexer) follows the compiler's rules. A `<` starts markup when it is
//! followed by a letter, `_` or `>` and the code token before it cannot end an operand, so
//! `a < b` and `#list < 3` stay comparisons while `return <view/>`, `f(<a/>)`, `{ <a/> }` and
//! `cond and <a/>` are markup. An element is a tag (`<name attributes>` or `<name ... />`),
//! a fragment (`<>...</>`), and for tags that are not self-closing the children up to the
//! matching closing tag: text, nested elements, `<!-- -->` comments and `{expression}`
//! splices. Attributes are `name="text"`, `name='text'`, `name={expression}`, a bare `name` and
//! `{...expression}` spreads. Lua inside a splice is ordinary Lua and may hold markup again.
//!
//! Markup becomes [`TokenKind::Markup`](crate::token::TokenKind::Markup) tokens. A markup token
//! is the markup between two pieces of Lua on one line: the leading indentation, the trailing
//! whitespace and the line break of every line inside markup are ordinary trivia tokens, so a
//! markup token spans several lines only when a quoted attribute value or a `<!-- -->` comment
//! does, and only a `<!--` comment left open at the end of the file ends with whitespace. The
//! `{`, `{...` and `}` around a splice end and start markup tokens, and the Lua between them is
//! lexed as usual. A markup token therefore either ends a whole element, ends with the `{` or
//! `{...` opening a splice ([`opens_splice`]), starts with the `}` closing one
//! ([`Markup::splice_ends`]), or starts or ends a line.

use std::ops::Range;

/// A tag name written in markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Byte range of the name, after `<` or `</` (`Flux.Panels.Card`, `view`).
    pub name: Range<usize>,
    /// Whether the name belongs to a closing tag (`</view>`).
    pub closing: bool,
}

/// Malformed markup, described with the words of the Lumen compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkupError {
    /// Byte range the problem is reported at.
    pub range: Range<usize>,
    /// What is wrong, as the compiler says it (`'<view>' is never closed`).
    pub message: String,
}

/// What the lexer learned about the markup of a `.lumen` file; empty for Lua files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Markup {
    /// Byte ranges of the outermost elements, from their `<` to the end of their closing tag.
    pub elements: Vec<Range<usize>>,
    /// Every tag name in source order, closing tags included.
    pub tags: Vec<Tag>,
    /// Byte offsets of the `}` closing every Lua splice, in source order. A line of text may
    /// start with a `}` as well, so the text of a markup token cannot tell.
    pub splice_ends: Vec<usize>,
    /// Malformed markup in source order. The lexer recovers from each problem, so problems
    /// after the first may be consequences of it.
    pub errors: Vec<MarkupError>,
}

/// Whether the text of a markup token ends with the `{` or `{...` that opens a Lua splice.
pub fn opens_splice(text: &str) -> bool {
    text.ends_with('{') || text.ends_with("{...")
}

/// Whether a tag name names a component, a Lua expression such as `Card` or `Flux.Panels.Card`,
/// rather than an intrinsic element or a template ID passed on as a string: the compiler takes
/// names starting with an upper case letter as components.
pub fn is_component_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
}

/// The Lua variable a component name reads: the part before the first `.` (`Flux` for
/// `Flux.Panels.Card`), or before the first `-` of a malformed name.
pub fn component_root(name: &str) -> &str {
    name.split(['.', '-']).next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_delimiters() {
        assert!(opens_splice("<view style={"));
        assert!(opens_splice("} key={"));
        assert!(opens_splice("<view {..."));
        assert!(!opens_splice("<view/>"));
    }

    #[test]
    fn component_names() {
        assert!(is_component_name("Command"));
        assert!(is_component_name("Flux.Panels.Card"));
        assert!(!is_component_name("view"));
        assert!(!is_component_name("help.page"));
        assert!(!is_component_name("_private"));
        assert_eq!(component_root("Flux.Panels.Card"), "Flux");
        assert_eq!(component_root("Command"), "Command");
        assert_eq!(component_root("My-card"), "My");
    }
}
