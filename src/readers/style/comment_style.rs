//! `Style/CommentStyle`.

use std::collections::HashSet;

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Checks that comments use the configured comment markers.
///
/// With `EnforcedStyle: lua` (the default) `// x` becomes `-- x` (a run of slashes becomes as
/// many dashes, so `/// Doc.` becomes `--- Doc.`) and `/* x */` becomes `--[[ x ]]`, with as many
/// `=` in the brackets (`--[==[ x ]==]`) as it takes for the body not to close the comment early.
///
/// With `EnforcedStyle: c` `-- x` becomes `// x` and `--[[ x ]]` (any level) becomes `/* x */`,
/// except where that would change the code or the documentation: doc comments stay as they are,
/// because LDoc and EmmyLua tooling (and the documentation readers) only read `--` comments. A doc
/// comment is a line comment starting with `---` together with every comment-only line around it
/// that has no blank or code line in between (`--- Summary.` and its `-- @param` lines). A block
/// comment whose body contains `*/` is left alone too.
///
/// Only the comment markers change; the comment text, including inline `proofreader-disable`
/// directives, stays the same. Markup comments of Lumen templates (`<!-- -->`) are markup and
/// never touched, and unterminated block comments are left to `Lint/Syntax`.
pub struct CommentStyle;

impl Reader for CommentStyle {
    fn name(&self) -> &'static str {
        "Style/CommentStyle"
    }

    fn description(&self) -> &'static str {
        "Checks that comments use the configured comment markers."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("lua".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let c_style = ctx.option_str("EnforcedStyle", "lua") == "c";
        let source = ctx.source;
        let documentation = if c_style {
            doc_comment_lines(source)
        } else {
            HashSet::new()
        };
        for token in source.code_tokens() {
            let TokenKind::Comment { long } = token.kind else {
                continue;
            };
            let text = source.text_of(token);
            let found = match (c_style, long) {
                (false, false) => to_dashes(token, text),
                (false, true) => to_long_brackets(token, text),
                (true, false) if !documentation.contains(&token.line) => to_slashes(token, text),
                (true, false) => None,
                (true, true) => to_c_block(token, text),
            };
            if let Some((message, edits)) = found {
                ctx.add_offense_with_fix(token.range(), message, edits);
            }
        }
    }
}

/// The lines of the doc comments of `source`: runs of consecutive lines holding nothing but a
/// line comment, at least one of which starts with `---`.
fn doc_comment_lines(source: &Source) -> HashSet<u32> {
    let mut lines = HashSet::new();
    let mut run: Vec<u32> = Vec::new();
    let mut documented = false;
    for number in 1..=source.line_count() + 1 {
        if let Some(comment) = comment_only_line(source, number) {
            run.push(number);
            documented |= source.text_of(comment).starts_with("---");
            continue;
        }
        if documented {
            lines.extend(run.iter().copied());
        }
        run.clear();
        documented = false;
    }
    lines
}

/// The comment token of line `n` when the line holds nothing but a line comment.
fn comment_only_line(source: &Source, n: u32) -> Option<&Token> {
    let first = source.first_code_token_on_line(n)?;
    let last = source.last_code_token_on_line(n)?;
    (first == last && first.kind == (TokenKind::Comment { long: false })).then_some(first)
}

/// The fix of a `//` line comment in the `lua` style: its leading slashes become dashes.
fn to_dashes(token: &Token, text: &str) -> Option<(String, Vec<Edit>)> {
    if !text.starts_with("//") {
        return None;
    }
    let slashes = text.len() - text.trim_start_matches('/').len();
    Some((
        "Use `--` instead of `//` for comments.".to_owned(),
        vec![Edit::replace(
            token.start..token.start + slashes,
            "-".repeat(slashes),
        )],
    ))
}

/// The fix of a `/* */` block comment in the `lua` style: long brackets of the lowest level the
/// body cannot close early.
fn to_long_brackets(token: &Token, text: &str) -> Option<(String, Vec<Edit>)> {
    if text.len() < 4 {
        return None;
    }
    let body = text.strip_prefix("/*")?.strip_suffix("*/")?;
    let level = (0..).map(|level| "=".repeat(level)).find(|equals| {
        let close = format!("]{equals}]");
        format!("{body}{close}").find(&close) == Some(body.len())
    })?;
    Some((
        "Use `--[[ ]]` instead of `/* */` for block comments.".to_owned(),
        vec![
            Edit::replace(token.start..token.start + 2, format!("--[{level}[")),
            Edit::replace(token.end - 2..token.end, format!("]{level}]")),
        ],
    ))
}

/// The fix of a `--` line comment in the `c` style; `---` doc comments are left alone.
fn to_slashes(token: &Token, text: &str) -> Option<(String, Vec<Edit>)> {
    if !text.starts_with("--") || text.starts_with("---") {
        return None;
    }
    Some((
        "Use `//` instead of `--` for comments.".to_owned(),
        vec![Edit::replace(token.start..token.start + 2, "//")],
    ))
}

/// The fix of a `--[[ ]]` block comment in the `c` style, unless its body contains `*/`.
fn to_c_block(token: &Token, text: &str) -> Option<(String, Vec<Edit>)> {
    let brackets = text.strip_prefix("--[")?;
    let equals = brackets.len() - brackets.trim_start_matches('=').len();
    let open = 4 + equals;
    let close = format!("]{}]", "=".repeat(equals));
    if text.get(open - 1..open) != Some("[")
        || text.len() < open + close.len()
        || !text.ends_with(&close)
    {
        return None;
    }
    let body = text.get(open..text.len() - close.len())?;
    if body.contains("*/") {
        return None;
    }
    Some((
        "Use `/* */` instead of `--[[ ]]` for block comments.".to_owned(),
        vec![
            Edit::replace(token.start..token.start + open, "/*"),
            Edit::replace(token.end - close.len()..token.end, "*/"),
        ],
    ))
}

inventory::submit! { Registration(|| Box::new(CommentStyle)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/CommentStyle";
    const C: &str = "EnforcedStyle: c";
    const LINE: &str = "Use `--` instead of `//` for comments.";
    const BLOCK: &str = "Use `--[[ ]]` instead of `/* */` for block comments.";
    const C_LINE: &str = "Use `//` instead of `--` for comments.";
    const C_BLOCK: &str = "Use `/* */` instead of `--[[ ]]` for block comments.";

    /// `(line, col, message)` of the offenses with `EnforcedStyle: c`.
    fn c_offenses(src: &str) -> Vec<(u32, u32, String)> {
        inspect_with(READER, src, C)
            .into_iter()
            .map(|offense| (offense.line, offense.col, offense.message))
            .collect()
    }

    /// `src` corrected with `EnforcedStyle: c`, checked to be idempotent.
    fn c_corrected(src: &str) -> String {
        let corrected = autocorrect_with(READER, src, C);
        assert_eq!(autocorrect_with(READER, &corrected, C), corrected);
        corrected
    }

    #[test]
    fn lua_style_accepts_lua_comments() {
        expect_no_offenses(
            READER,
            "-- a\n--- Doc.\n-- @param x\nx = 1 -- b\n--[[ c ]]\n--[==[ d ]==]\n----------\n",
        );
        expect_no_offenses(READER, "s = '// not a comment /* */'\n");
    }

    #[test]
    fn lua_style_flags_c_comments() {
        expect_offenses(
            READER,
            "// a\nx = 1 // b\n/* c */\n",
            &[(1, 1, LINE), (2, 7, LINE), (3, 1, BLOCK)],
        );
    }

    #[test]
    fn lua_style_autocorrects() {
        expect_correction(
            READER,
            "// a\n/// Doc.\n//////\nx = 1 //b\n/* c\n d */\n",
            "-- a\n--- Doc.\n------\nx = 1 --b\n--[[ c\n d ]]\n",
        );
        expect_correction(READER, "/* t[a[1]] */\n", "--[=[ t[a[1]] ]=]\n");
        expect_correction(READER, "/* a] */\n", "--[[ a] ]]\n");
        expect_correction(READER, "/*a]*/\n", "--[=[a]]=]\n");
        expect_correction(READER, "/**/\n", "--[[]]\n");
    }

    #[test]
    fn unterminated_block_comments_are_left_to_lint_syntax() {
        expect_no_offenses(READER, "/* open\n");
        assert!(c_offenses("--[[ open\n").is_empty());
    }

    #[test]
    fn c_style_flags_lua_comments() {
        assert_eq!(
            c_offenses("-- a\nx = 1 -- b\n--[[ c ]]\n--[==[ d ]==]\n// e\n/* f */\n"),
            vec![
                (1, 1, C_LINE.to_owned()),
                (2, 7, C_LINE.to_owned()),
                (3, 1, C_BLOCK.to_owned()),
                (4, 1, C_BLOCK.to_owned())
            ]
        );
    }

    #[test]
    fn c_style_autocorrects() {
        assert_eq!(
            c_corrected("-- a\nx = 1 --b\n--[[ c\n d ]]\n--[==[ e ]] ]==]\n"),
            "// a\nx = 1 //b\n/* c\n d */\n/* e ]] */\n"
        );
    }

    #[test]
    fn c_style_keeps_doc_comment_blocks() {
        let src = "--- Summary.\n-- More text.\n-- @param x [Number]\n-- @return [Number]\n\
                   function f(x) return x end\n\n---@param y number\n-- note\nlocal function g(y) \
                   return y end\n";
        assert!(c_offenses(src).is_empty(), "{:?}", c_offenses(src));
        assert_eq!(
            c_corrected("-- Plain.\n-- @param x\nfunction f(x) end\n"),
            "// Plain.\n// @param x\nfunction f(x) end\n"
        );
    }

    #[test]
    fn c_style_doc_blocks_end_at_blank_and_code_lines() {
        let src =
            "-- before\n\n--- Doc.\n-- @param x\nfunction f(x) -- trailing\n  -- inner\nend\n";
        assert_eq!(
            c_corrected(src),
            "// before\n\n--- Doc.\n-- @param x\nfunction f(x) // trailing\n  // inner\nend\n"
        );
        assert_eq!(
            c_corrected("x = 1 --- trailing doc\n-- after\n"),
            "x = 1 --- trailing doc\n// after\n"
        );
    }

    #[test]
    fn c_style_keeps_block_comments_holding_their_closer() {
        assert!(c_offenses("--[[ a */ b ]]\n").is_empty());
    }

    #[test]
    fn keeps_directives_working() {
        assert_eq!(
            c_corrected("x = 1 -- proofreader-disable-line Style/Not\n"),
            "x = 1 // proofreader-disable-line Style/Not\n"
        );
    }

    #[test]
    fn never_touches_markup_comments() {
        expect_no_offenses_lumen(READER, "return <view>\n  <!-- // a /* b */ -->\n</view>\n");
        expect_correction_lumen(
            READER,
            "// c\nreturn <view><!-- // d --></view>\n",
            "-- c\nreturn <view><!-- // d --></view>\n",
        );
    }
}
