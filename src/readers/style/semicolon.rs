//! `Style/Semicolon`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::block_structure::last_code_token_on_line;
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, StatementKind, Visitor, parse, walk,
};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags semicolons.
///
/// A semicolon ending a statement is removed together with the spaces around it; one that
/// separates two statements on a line becomes a single space, except before `(`, where removing
/// it would turn the next statement into a call and the offense is left uncorrected. A
/// semicolon separating table fields becomes a comma. Table separators are found with the syntax
/// tree, so a statement semicolon in a function inside a table stays a statement semicolon; in a
/// file that does not parse, a semicolon directly inside braces is taken as a table separator.
///
/// With `EnforcedStyle: require` (the Clockwork style) every statement that ends a line must end
/// with `;`: assignments, calls, `return`, `break` and the `end` (or `until ...`) closing a block
/// statement, function definitions included (`end;`, `end);`). Lines ending in `then`, `do`,
/// `else` or a function header end no statement and take none. A `;` may also end a statement in
/// the middle of a line (`if x then return; end;`); any other statement semicolon (`then;`,
/// `;;`) is flagged and removed. The fix inserts the missing `;` right after the statement,
/// before a trailing comment. Files that do not parse are only checked for table separators.
pub struct Semicolon;

impl Reader for Semicolon {
    fn name(&self) -> &'static str {
        "Style/Semicolon"
    }

    fn description(&self) -> &'static str {
        "Checks for semicolons, or with `EnforcedStyle: require` for statements without one."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("omit".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let separators = table_separators(source);
        let require = ctx.option_str("EnforcedStyle", "omit") == "require";
        let terminators = if require {
            match parse(source) {
                Ok(block) => Some(check_required(ctx, source, &block)),
                Err(_) => None,
            }
        } else {
            None
        };
        let mut open = Vec::new();
        for (code_index, token) in source.code_tokens().enumerate() {
            match token.kind {
                TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => {
                    open.push(token.kind)
                }
                TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                    open.pop();
                }
                TokenKind::Semicolon
                    if separators.as_ref().map_or_else(
                        || open.last() == Some(&TokenKind::LBrace),
                        |found| found.binary_search(&token.start).is_ok(),
                    ) =>
                {
                    ctx.add_offense_with_fix(
                        token.range(),
                        "Use `,` instead of `;` to separate table fields.",
                        vec![Edit::replace(token.range(), ",")],
                    );
                }
                TokenKind::Semicolon if require => {
                    let Some(allowed) = &terminators else {
                        continue;
                    };
                    if allowed.binary_search(&token.start).is_ok() {
                        continue;
                    }
                    let index = source.code_token_indexes()[code_index];
                    let offense = ctx.add_offense(token.range(), "Unnecessary semicolon.");
                    if let Some(edit) = statement_fix(source, index) {
                        offense.with_fix(vec![edit]);
                    }
                }
                TokenKind::Semicolon => {
                    let index = source.code_token_indexes()[code_index];
                    let offense = ctx.add_offense(token.range(), "Do not use semicolons.");
                    if let Some(edit) = statement_fix(source, index) {
                        offense.with_fix(vec![edit]);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Reports the statements of `block` (nested blocks included) that end a line without `;`, and
/// returns the start offsets of the semicolons that end a statement, sorted.
fn check_required(ctx: &mut Context, source: &Source, block: &Block) -> Vec<usize> {
    struct Blocks<'t>(Vec<&'t Block>);
    impl<'t> Visitor<'t> for Blocks<'t> {
        fn enter_block(&mut self, block: &'t Block) {
            self.0.push(block);
        }
    }
    let mut blocks = Blocks(Vec::new());
    walk(block, &mut blocks);
    let mut terminators = Vec::new();
    for block in blocks.0 {
        let mut previous_ends_statement = false;
        for (index, statement) in block.statements.iter().enumerate() {
            if statement.kind == StatementKind::Empty {
                if previous_ends_statement {
                    terminators.push(statement.range.start);
                }
                previous_ends_statement = false;
                continue;
            }
            previous_ends_statement = true;
            let terminated = block
                .statements
                .get(index + 1)
                .is_some_and(|next| next.kind == StatementKind::Empty);
            if terminated {
                continue;
            }
            let end = statement.range.end;
            let (line, _) = source.position(end.saturating_sub(1));
            if last_code_token_on_line(source, line).is_some_and(|last| last.end == end) {
                let last = source
                    .token_at(end.saturating_sub(1))
                    .map_or(end.saturating_sub(1)..end, |token| token.range());
                ctx.add_offense_with_fix(
                    last,
                    "Missing semicolon at the end of the statement.",
                    vec![Edit::insert(end, ";")],
                );
            }
        }
    }
    terminators.sort_unstable();
    terminators
}

/// Start offsets of the `;` separating table fields, sorted, or `None` when the file does not
/// parse.
fn table_separators(source: &Source) -> Option<Vec<usize>> {
    struct Separators(Vec<usize>);
    impl<'t> Visitor<'t> for Separators {
        fn expression(&mut self, expression: &'t Expression) {
            if let ExpressionKind::Table(table) = &expression.kind {
                self.0.extend(
                    table
                        .fields
                        .iter()
                        .filter_map(|field| field.separator)
                        .filter(|separator| separator.kind == TokenKind::Semicolon)
                        .map(|separator| separator.start),
                );
            }
        }
    }
    let block = parse(source).ok()?;
    let mut separators = Separators(Vec::new());
    walk(&block, &mut separators);
    separators.0.sort_unstable();
    Some(separators.0)
}

/// The edit removing the statement semicolon at `source.tokens[index]`, or `None` when the
/// next statement starts with `(`.
fn statement_fix(source: &Source, index: usize) -> Option<Edit> {
    let tokens = &source.tokens;
    let semicolon = &tokens[index];
    let is_space = |kind: TokenKind| matches!(kind, TokenKind::Space | TokenKind::Tab);
    let spaces_before = tokens[..index]
        .iter()
        .rev()
        .take_while(|token| is_space(token.kind))
        .count();
    let indented = index == spaces_before || tokens[index - spaces_before - 1].is_trivia();
    let start = if indented {
        semicolon.start
    } else {
        tokens[index - spaces_before].start
    };
    let end = tokens[index + 1..]
        .iter()
        .take_while(|token| is_space(token.kind))
        .last()
        .map_or(semicolon.end, |space| space.end);
    let next = source
        .token_at(end)
        .map_or(TokenKind::Eof, |token| token.kind);
    match next {
        TokenKind::LParen => None,
        TokenKind::Newline | TokenKind::Eof => Some(Edit::remove(start..end)),
        TokenKind::Comment { .. } => Some(Edit::remove(start..semicolon.end)),
        _ if indented => Some(Edit::remove(semicolon.start..end)),
        _ => Some(Edit::replace(start..end, " ")),
    }
}

inventory::submit! { Registration(|| Box::new(Semicolon)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/Semicolon";
    const MESSAGE: &str = "Do not use semicolons.";
    const TABLE: &str = "Use `,` instead of `;` to separate table fields.";

    #[test]
    fn flags_statement_semicolons() {
        expect_offense(READER, "x = 1;\n", 1, 6, MESSAGE);
        expect_offense(READER, "a(); b()\n", 1, 4, MESSAGE);
        expect_offense(READER, "return;\n", 1, 7, MESSAGE);
        expect_offense(READER, "f(function() a(); end)\n", 1, 17, MESSAGE);
    }

    #[test]
    fn flags_table_separators() {
        expect_offense(READER, "t = { a = 1; b = 2 }\n", 1, 12, TABLE);
        expect_offense(READER, "t = { f(x); [k] = v }\n", 1, 11, TABLE);
        expect_correction(READER, "t = { a = 1; b = 2 }\n", "t = { a = 1, b = 2 }\n");
    }

    #[test]
    fn statements_in_functions_inside_tables_are_statements() {
        let src = "local t = { f = function(v) local a = 1; return a end; g = 2 }\n";
        expect_offenses(READER, src, &[(1, 40, MESSAGE), (1, 54, TABLE)]);
        expect_correction(
            READER,
            src,
            "local t = { f = function(v) local a = 1 return a end, g = 2 }\n",
        );
    }

    #[test]
    fn falls_back_to_braces_when_the_file_does_not_parse() {
        expect_offense(READER, "t = { a = 1; b = 2 }\nif x\n", 1, 12, TABLE);
    }

    #[test]
    fn require_style() {
        let options = "EnforcedStyle: require";
        let src = "local x = 1\nif x then\n  f(x) -- c\nend\n\nfunction A:B()\n  return x\nend\n\n\
                   hook.Add('A', 'b', function()\n  g()\nend)\nt = {\n  a = 1\n}\n\
                   if x then return end\nrepeat x = x - 1 until x\n";
        assert_eq!(
            autocorrect_with(READER, src, options),
            "local x = 1;\nif x then\n  f(x); -- c\nend;\n\nfunction A:B()\n  return x;\nend;\n\n\
             hook.Add('A', 'b', function()\n  g();\nend);\nt = {\n  a = 1\n};\n\
             if x then return end;\nrepeat x = x - 1 until x;\n"
        );
        assert!(inspect_with(READER, "if x then return; end;\nf(); g();\n", options).is_empty());
    }

    #[test]
    fn require_style_removes_stray_semicolons() {
        let options = "EnforcedStyle: require";
        let offenses = inspect_with(READER, "if x then; f(); end;\ng();;\n", options);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 10, "Unnecessary semicolon."),
                (2, 5, "Unnecessary semicolon.")
            ]
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "if x then; f(); end;\ng();;\nt = { a = 1; b = 2 };\n",
                options
            ),
            "if x then f(); end;\ng();\nt = { a = 1, b = 2 };\n"
        );
    }

    #[test]
    fn accepts_code_without_semicolons() {
        expect_no_offenses(READER, "x = 1\ns = ';'\n-- a; b\n");
    }

    #[test]
    fn autocorrects_statement_semicolons() {
        expect_correction(READER, "x = 1;\n", "x = 1\n");
        expect_correction(READER, "x = 1 ;  \n", "x = 1\n");
        expect_correction(READER, "x = 1;", "x = 1");
        expect_correction(READER, "a();b()\n", "a() b()\n");
        expect_correction(READER, "a() ;  b()\n", "a() b()\n");
        expect_correction(READER, "a(); -- c\n", "a() -- c\n");
        expect_correction(READER, "  ; b()\n", "  b()\n");
        expect_correction(READER, "x = 1;\r\n", "x = 1\r\n");
    }

    #[test]
    fn leaves_ambiguous_calls_alone() {
        let offenses = inspect(READER, "a = b; (f or g)()\n");
        assert_eq!(offenses.len(), 1);
        assert!(!offenses[0].correctable());
        expect_correction(READER, "a = b; (f or g)()\n", "a = b; (f or g)()\n");
    }

    #[test]
    fn markup_text_is_not_code() {
        expect_no_offenses_lumen(
            READER,
            "return <text style={{ a = 1 }}>a; b &amp; c</text>\n",
        );
    }
}
