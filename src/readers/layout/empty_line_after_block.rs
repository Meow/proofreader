//! `Layout/EmptyLineAfterBlock`.

use super::block_structure::{
    BlockStructure, LineKind, insert_blank_lines_after, last_code_token_on_line,
};
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags a line that only closes a multi-line block with `end` (possibly followed by more
/// closing brackets and `end`s) when the next line is neither blank nor another closer.
///
/// No blank line is needed when the next line ends an indented group by returning to the
/// indentation of the group's block (`  end` / `cam.End3D2D()`).
///
/// A line ending with a comma (`end,` or `end),`) separates items of a table or an argument
/// list, which Flux never splits with blank lines, so it is not checked.
pub struct EmptyLineAfterBlock;

impl Reader for EmptyLineAfterBlock {
    fn name(&self) -> &'static str {
        "Layout/EmptyLineAfterBlock"
    }

    fn description(&self) -> &'static str {
        "Checks for an empty line after the end of a multi-line block."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        for line in 1..structure.line_count() {
            let Some(end) = structure.first_token(line) else {
                continue;
            };
            if structure.kind(line) != LineKind::Closer || end.kind != TokenKind::End {
                continue;
            }
            let Some(block) = structure
                .closed_block(line)
                .map(|block| structure.block(block))
                .filter(|block| block.opener.line < line)
            else {
                continue;
            };
            if !only_closers_on_line(source, line) {
                continue;
            }
            let next = line + 1;
            if source.is_blank(next)
                || matches!(structure.kind(next), LineKind::Closer | LineKind::Skipped)
                || structure.is_group_edge(
                    source,
                    next,
                    structure.statement_line(block.anchor_line),
                )
            {
                continue;
            }
            let Some(edit) = insert_blank_lines_after(source, line, 1) else {
                continue;
            };
            ctx.add_offense_with_fix(end.range(), "Add empty line after block.", vec![edit]);
        }
    }
}

/// Whether the code on line `n` consists of closers and semicolons only.
fn only_closers_on_line(source: &Source, n: u32) -> bool {
    let Some(last) = last_code_token_on_line(source, n) else {
        return false;
    };
    if last.kind == TokenKind::Comma {
        return false;
    }
    source
        .code_tokens_on_line(n)
        .filter(|token| !token.kind.is_comment())
        .all(|token| {
            token.end_line == last.end_line
                && matches!(
                    token.kind,
                    TokenKind::End
                        | TokenKind::RParen
                        | TokenKind::RBrace
                        | TokenKind::RBracket
                        | TokenKind::Semicolon
                )
        })
}

inventory::submit! { Registration(|| Box::new(EmptyLineAfterBlock)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLineAfterBlock";
    const MESSAGE: &str = "Add empty line after block.";

    #[test]
    fn accepts_blank_lines_closers_and_eof() {
        expect_no_offenses(
            READER,
            "if a then\n  b()\nend\n\nx()\nfunction f()\n  if a then\n    b()\n  end\nend\n",
        );
        expect_no_offenses(READER, "if a then\n  for i = 1, 2 do\n  end\nelse\nend");
        expect_no_offenses(READER, "foo(function()\n  x()\nend)\n");
        expect_no_offenses(READER, "local t = {\n  f = function()\n  end\n}\n\n\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_code_right_after_end() {
        expect_offense(READER, "if a then\n  b()\nend\nx()\n", 3, 1, MESSAGE);
        expect_offense(
            READER,
            "hook.Add('X', 'y', function()\n  b()\nend)\nx()\n",
            3,
            1,
            MESSAGE,
        );
        expect_offense(
            READER,
            "function f()\n  while a do\n    b()\n  end\n  x()\nend\n",
            4,
            3,
            MESSAGE,
        );
        expect_offense(READER, "if a then\n  b()\nend\n-- note\n", 3, 1, MESSAGE);
    }

    #[test]
    fn accepts_the_end_of_an_indented_group() {
        expect_no_offenses(
            READER,
            "function f()\n  cam.Start3D2D(pos, ang, 1)\n    if a then\n      b()\n    end\n  cam.End3D2D()\nend\n",
        );
        expect_no_offenses(
            READER,
            "function f()\n  local query = q()\n    query:callback(function()\n      b()\n    end)\n  query:execute()\nend\n",
        );
    }

    #[test]
    fn ignores_one_line_blocks_and_other_closers() {
        expect_no_offenses(READER, "if a then b() end\nx()\n");
        expect_no_offenses(READER, "foo(function()\nend, function()\nend)\n");
        expect_no_offenses(READER, "repeat\n  x()\nuntil y\nz()\n");
        expect_no_offenses(READER, "local t = {\n  a = 1\n}\nx()\n");
        expect_no_offenses(READER, "if a then\n  b()\nend x()\ny()\n");
    }

    #[test]
    fn ignores_items_separated_by_commas() {
        expect_no_offenses(
            READER,
            "local t = {\n  f = function()\n  end,\n  g = 1\n}\n",
        );
        expect_no_offenses(READER, "foo(function()\n  x()\nend),\ny\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "if a then\n  b()\nend\nx()\n",
            "if a then\n  b()\nend\n\nx()\n",
        );
        expect_correction(
            READER,
            "foo(function()\n  x()\nend)\ny()\n",
            "foo(function()\n  x()\nend)\n\ny()\n",
        );
    }
}
