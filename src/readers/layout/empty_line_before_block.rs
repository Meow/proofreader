//! `Layout/EmptyLineBeforeBlock`.

use yaml_rust2::Yaml;

use super::block_structure::{BlockKind, BlockStructure, LineKind, insert_blank_lines_after};
use crate::reader::{Context, Reader, Registration};

/// Flags multi-line block statements (`if`, `for`, `while`, `repeat`, `do` and named function
/// definitions) not preceded by a blank line. With `IncludeAnonymousFunctions`, statements
/// that only open a multi-line anonymous function (`btn.DoClick = function(b)`,
/// `hook.Add('X', 'y', function()`) are checked too.
///
/// No blank line is needed for the first statement of a block or of the file, for the first
/// statement of an indented group, or after a comment line.
pub struct EmptyLineBeforeBlock;

impl Reader for EmptyLineBeforeBlock {
    fn name(&self) -> &'static str {
        "Layout/EmptyLineBeforeBlock"
    }

    fn description(&self) -> &'static str {
        "Checks for an empty line before multi-line block statements."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("IncludeAnonymousFunctions", Yaml::Boolean(false))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let include_anonymous = ctx.option_bool("IncludeAnonymousFunctions", false);
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        let first_code_line = (1..=structure.line_count()).find(|&line| structure.has_code(line));
        for line in 2..=structure.line_count() {
            if !structure.is_statement_start(line) {
                continue;
            }
            let definition = structure.definition(source, line);
            let Some(block) = structure.multiline_blocks_on_line(line).find(|&block| {
                include_anonymous
                    || structure.block(block).kind != BlockKind::Function
                    || definition == Some(block)
            }) else {
                continue;
            };
            let previous = line - 1;
            if matches!(
                structure.kind(previous),
                LineKind::Blank | LineKind::Comment
            ) {
                continue;
            }
            let first_in_block = match structure.innermost(line) {
                Some(parent) => structure.block(parent).first_body_line == Some(line),
                None => first_code_line == Some(line),
            };
            if first_in_block
                || structure.is_group_edge(source, structure.statement_line(previous), line)
            {
                continue;
            }
            let opener = structure.block(block).opener;
            let Some(edit) = insert_blank_lines_after(source, previous, 1) else {
                continue;
            };
            ctx.add_offense_with_fix(
                opener.range(),
                format!("Add empty line before `{}` block.", source.text_of(&opener)),
                vec![edit],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(EmptyLineBeforeBlock)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLineBeforeBlock";

    #[test]
    fn accepts_blocks_after_blank_lines() {
        expect_no_offenses(
            READER,
            "local a = 1\n\nif a then\n  b()\nend\n\nfor i = 1, 2 do\nend\n",
        );
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_blocks_directly_after_statements() {
        expect_offense(
            READER,
            "local a = 1\nif a then\n  b()\nend\n",
            2,
            1,
            "Add empty line before `if` block.",
        );
        expect_offense(
            READER,
            "x()\nwhile y do\n  z()\nend\n",
            2,
            1,
            "Add empty line before `while` block.",
        );
        expect_offense(
            READER,
            "x()\nlocal function f()\nend\n",
            2,
            7,
            "Add empty line before `function` block.",
        );
        expect_offense(
            READER,
            "x()\nrepeat\n  y()\nuntil z\n",
            2,
            1,
            "Add empty line before `repeat` block.",
        );
        expect_offense(
            READER,
            "x()\ndo\n  y()\nend\n",
            2,
            1,
            "Add empty line before `do` block.",
        );
    }

    #[test]
    fn ignores_anonymous_functions_by_default() {
        expect_no_offenses(
            READER,
            "x()\nconcommand.Add('x', function(actor)\n  y()\nend)\nself.btn.DoClick = function(btn)\n  z()\nend\n",
        );
        expect_no_offenses(READER, "x()\nlocal f = function()\n  y()\nend\n");
    }

    #[test]
    fn checks_keyword_blocks_after_an_anonymous_function_on_the_line() {
        expect_offense(
            READER,
            "x()\nfor _, f in ipairs({ function() end }) do\n  f()\nend\n",
            2,
            1,
            "Add empty line before `for` block.",
        );
    }

    #[test]
    fn can_include_anonymous_functions() {
        let options = "IncludeAnonymousFunctions: true";
        let offenses = inspect_with(
            READER,
            "x()\nconcommand.Add('x', function(actor)\n  y()\nend)\n",
            options,
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (2, 21));
        assert_eq!(
            offenses[0].message,
            "Add empty line before `function` block."
        );
        let offenses = inspect_with(
            READER,
            "x()\nself.btn.DoClick = function(btn)\n  z()\nend\n",
            options,
        );
        assert_eq!(offenses.len(), 1);
    }

    #[test]
    fn accepts_blocks_opening_an_indented_group() {
        expect_no_offenses(
            READER,
            "function f()\n  cam.Start3D2D(pos, ang, 1)\n    if a then\n      b()\n    end\n  cam.End3D2D()\nend\n",
        );
        expect_no_offenses(
            READER,
            "local ret = {}\n  for k, v in pairs(t) do\n    ret[k] = v\n  end\nreturn ret\n",
        );
    }

    #[test]
    fn flags_blocks_inside_an_indented_group() {
        expect_offense(
            READER,
            "local ret = {}\n  x()\n  for k, v in pairs(t) do\n    ret[k] = v\n  end\nreturn ret\n",
            3,
            3,
            "Add empty line before `for` block.",
        );
    }

    #[test]
    fn flags_blocks_after_block_ends() {
        expect_offense(
            READER,
            "if a then\n  b()\nend\nif c then\n  d()\nend\n",
            4,
            1,
            "Add empty line before `if` block.",
        );
    }

    #[test]
    fn allows_first_statements_and_comments() {
        expect_no_offenses(READER, "if a then\n  b()\nend\n");
        expect_no_offenses(READER, "-- c\n\nif a then\n  b()\nend\n");
        expect_no_offenses(
            READER,
            "function f()\n  if a then\n    for i = 1, 2 do\n    end\n  else\n    while x do\n    end\n  end\nend\n",
        );
        expect_no_offenses(READER, "x()\n--- Doc.\nfunction f()\nend\n");
        expect_no_offenses(
            READER,
            "function f(a,\n           b)\n  if a then\n  end\nend\n",
        );
        expect_no_offenses(
            READER,
            "if a then\n  b()\nelseif c then\n  if d then\n  end\nend\n",
        );
    }

    #[test]
    fn ignores_one_line_blocks_and_items() {
        expect_no_offenses(
            READER,
            "x()\nif a then return end\nlocal f = function() return 1 end\n",
        );
        expect_no_offenses(
            READER,
            "local t = {\n  a = 1,\n  b = function()\n  end\n}\n",
        );
        expect_no_offenses(READER, "local t = {\n  a = 1\n}\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "local a = 1\nif a then\n  b()\nend\n",
            "local a = 1\n\nif a then\n  b()\nend\n",
        );
        expect_correction(
            READER,
            "x()\r\nfor i = 1, 2 do\r\nend\r\n",
            "x()\r\n\r\nfor i = 1, 2 do\r\nend\r\n",
        );
    }
}
