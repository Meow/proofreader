//! `Layout/IndentationWidth`.

use yaml_rust2::Yaml;

use super::block_structure::{BlockStructure, LineKind, indent_width, indentation_range};
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;

/// Flags block bodies whose first line is not indented by `Width` more than the line that opens
/// the block, and closers that are not aligned with that line.
///
/// A body or closer aligned with the line where the opener's statement starts is accepted as
/// well, for openers that sit on a continuation line. Continuation lines are never checked.
pub struct IndentationWidth;

impl Reader for IndentationWidth {
    fn name(&self) -> &'static str {
        "Layout/IndentationWidth"
    }

    fn description(&self) -> &'static str {
        "Checks that block bodies are indented by the configured width and closers align with their openers."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("Width", Yaml::Integer(2))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let width = ctx.option_usize("Width", 2);
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        check_bodies(ctx, source, &structure, width);
        check_closers(ctx, source, &structure);
    }
}

/// Indentation widths a line hanging off `anchor` may be measured from: the anchor itself first,
/// then the start of the statement holding it. `None` when the anchor cannot be measured.
fn bases(source: &Source, structure: &BlockStructure, anchor: u32) -> Option<Vec<usize>> {
    if structure.kind(anchor) == LineKind::Skipped {
        return None;
    }
    let base = indent_width(source, anchor)?;
    let statement = indent_width(source, structure.statement_line(anchor)).unwrap_or(base);
    Some(vec![base, statement])
}

/// Reports first body lines that are not indented by `width`.
fn check_bodies(ctx: &mut Context, source: &Source, structure: &BlockStructure, width: usize) {
    for block in structure.blocks() {
        let Some(line) = block.first_body_line.filter(|_| block.opens_line) else {
            continue;
        };
        if structure.kind(line) != LineKind::Statement {
            continue;
        }
        let (Some(actual), Some(first)) = (indent_width(source, line), structure.first_token(line))
        else {
            continue;
        };
        let Some(mut bases) = bases(source, structure, block.anchor_line) else {
            continue;
        };
        if let Some(body_line) = block.body_start.map(|token| token.line)
            && structure.kind(body_line) != LineKind::Skipped
            && let Some(width) = indent_width(source, body_line)
        {
            bases.push(width);
        }
        if bases.iter().any(|base| actual == base + width) {
            continue;
        }
        let base = bases[0];
        let relative = actual as i64 - base as i64;
        ctx.add_offense_with_fix(
            indentation_range(source, line, first),
            format!("Use {width} (not {relative}) spaces for indentation."),
            structure.reindent(source, line, base + width),
        );
    }
}

/// Reports closer lines that are not aligned with the line opening their block.
fn check_closers(ctx: &mut Context, source: &Source, structure: &BlockStructure) {
    for line in 1..=structure.line_count() {
        if structure.kind(line) != LineKind::Closer {
            continue;
        }
        let Some(block) = structure
            .closed_block(line)
            .map(|index| structure.block(index))
        else {
            continue;
        };
        if block.kind.is_bracket() && !block.opens_line {
            continue;
        }
        let (Some(actual), Some(first)) = (indent_width(source, line), structure.first_token(line))
        else {
            continue;
        };
        let Some(bases) = bases(source, structure, block.anchor_line) else {
            continue;
        };
        if bases.contains(&actual) {
            continue;
        }
        let base = bases[0];
        ctx.add_offense_with_fix(
            indentation_range(source, line, first),
            format!(
                "Align `{}` with the start of line {}.",
                source.text_of(&first),
                block.anchor_line
            ),
            structure.reindent(source, line, base),
        );
    }
}

inventory::submit! { Registration(|| Box::new(IndentationWidth)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/IndentationWidth";

    #[test]
    fn accepts_two_space_bodies() {
        expect_no_offenses(
            READER,
            "function f(a)\n  if a then\n    for i = 1, 2 do\n      x()\n    end\n  elseif b then\n    y()\n  else\n    z()\n  end\nend\n",
        );
        expect_no_offenses(
            READER,
            "while x do\n  y()\nend\nrepeat\n  z()\nuntil w\ndo\n  v()\nend\n",
        );
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_body_indentation() {
        expect_offense(
            READER,
            "if x then\n    y()\nend\n",
            2,
            1,
            "Use 2 (not 4) spaces for indentation.",
        );
        expect_offense(
            READER,
            "function f()\ny()\nend\n",
            2,
            1,
            "Use 2 (not 0) spaces for indentation.",
        );
        expect_offense(
            READER,
            "  if x then\ny()\n  end\n",
            2,
            1,
            "Use 2 (not -2) spaces for indentation.",
        );
    }

    #[test]
    fn flags_misaligned_closers() {
        expect_offense(
            READER,
            "if x then\n  y()\n  end\n",
            3,
            1,
            "Align `end` with the start of line 1.",
        );
        expect_offense(
            READER,
            "if x then\n  y()\n else\n  z()\nend\n",
            3,
            1,
            "Align `else` with the start of line 1.",
        );
        expect_offense(
            READER,
            "local t = {\n  a = 1\n  }\n",
            3,
            1,
            "Align `}` with the start of line 1.",
        );
        expect_offense(
            READER,
            "repeat\n  x()\n  until y\n",
            3,
            1,
            "Align `until` with the start of line 1.",
        );
    }

    #[test]
    fn honours_width() {
        let offenses = inspect_with(READER, "if x then\n  y()\nend\n", "Width: 4");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, "Use 4 (not 2) spaces for indentation.");
        assert_eq!(
            autocorrect_with(READER, "if x then\n  y()\nend\n", "Width: 4"),
            "if x then\n    y()\nend\n"
        );
    }

    #[test]
    fn measures_from_the_opening_keyword_line() {
        expect_no_offenses(READER, "if a and\n   b then\n  c()\nend\n");
        expect_no_offenses(READER, "function f(a,\n           b)\n  return a\nend\n");
        expect_no_offenses(
            READER,
            "local x = foo(a,\n              function()\n                y()\n              end)\n",
        );
        expect_no_offenses(READER, "foo(a,\n    b, function()\n  y()\nend)\n");
        expect_no_offenses(
            READER,
            "if a\n  or b\n  or c then\n    d()\nelseif e\n  or f then\n    g()\nend\n",
        );
    }

    #[test]
    fn brackets_ending_their_line() {
        expect_no_offenses(
            READER,
            "local t = {\n  'a',\n  b = {\n    c = 1\n  }\n}\nfoo(\n  1,\n  2\n)\nbar({\n  x = 1\n})\n",
        );
        expect_offense(
            READER,
            "foo(\n    1\n)\n",
            2,
            1,
            "Use 2 (not 4) spaces for indentation.",
        );
    }

    #[test]
    fn ignores_continuation_lines() {
        expect_no_offenses(
            READER,
            "local s = str\n             :gsub('a', 'b')\nlocal t = foo(a,\n              b)\nif x and\n     y then\n  z()\nend\nlocal u = { a,\n  b }\nfoo(a, b\n)\n",
        );
        expect_no_offenses(READER, "error(\n  'a'..\n  tostring(b)..\n      'c'\n)\n");
    }

    #[test]
    fn ignores_one_line_and_empty_blocks() {
        expect_no_offenses(
            READER,
            "if x then return end\nfunction f()\nend\nlocal t = {\n}\n",
        );
        expect_no_offenses(READER, "if x then y()\n  z()\nend\n");
    }

    #[test]
    fn ignores_multiline_strings_and_tabs() {
        expect_no_offenses(
            READER,
            "function f()\n  local s = [[\nx\n      y]]\n  return s\nend\n",
        );
        expect_no_offenses(READER, "function f()\n\ty()\nend\n");
        expect_no_offenses(READER, "function f()\n  -- comment\n  y()\nend\n");
        expect_no_offenses(READER, "function f()\n--[[\n  x\n]]\n  y()\nend\n");
    }

    #[test]
    fn autocorrects_the_line_and_its_continuations() {
        expect_correction(
            READER,
            "function f()\n    local x = a and\n      b\n  c()\nend\n",
            "function f()\n  local x = a and\n    b\n  c()\nend\n",
        );
        expect_correction(READER, "if x then\ny()\n  end\n", "if x then\n  y()\nend\n");
        expect_correction(
            READER,
            "if x then\n  if y then\n  z()\n  end\nend\n",
            "if x then\n  if y then\n    z()\n  end\nend\n",
        );
    }
}
