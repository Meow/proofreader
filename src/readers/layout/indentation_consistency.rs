//! `Layout/IndentationConsistency`.

use std::collections::{HashMap, HashSet};

use yaml_rust2::Yaml;

use super::block_structure::{BlockStructure, LineKind, indent_width, indentation_range};
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::Token;

/// Flags statement lines indented differently from the first statement line of their block.
///
/// Items of a bracket whose opener ends its line count as statements of that bracket;
/// continuation lines, closers and lines inside multi-line strings or comments are ignored.
/// A line indented deeper than the reference is accepted when its first token lines up with a
/// token of the line above (column-aligned tables). With `AllowIndentedGroups` (the default), a
/// run of statements may be indented exactly `IndentationWidth` deeper than the block's
/// reference to form a visual group (`net.Start(id)` / `  net.WriteString(s)` / `net.Send(target)`),
/// as long as a statement at the reference level follows the group in the same block. Items of
/// a table constructor or an argument list never form groups. A line indented deeper than an
/// open group is corrected to the group's level, any other line to the block's reference.
pub struct IndentationConsistency;

/// Indentation state of one block while its statement lines are visited.
struct BlockIndent {
    /// Indentation width of the block's first statement line.
    base: usize,
    /// Lines of the indented group currently open, with their first token.
    group: Vec<(u32, Token)>,
}

impl Reader for IndentationConsistency {
    fn name(&self) -> &'static str {
        "Layout/IndentationConsistency"
    }

    fn description(&self) -> &'static str {
        "Checks that the statements of a block are indented consistently."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("AllowIndentedGroups", Yaml::Boolean(true)),
            ("IndentationWidth", Yaml::Integer(2)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let allow_groups = ctx.option_bool("AllowIndentedGroups", true);
        let group_width = ctx.option_usize("IndentationWidth", 2);
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        let mut blocks: HashMap<Option<usize>, BlockIndent> = HashMap::new();
        let mut aligned = HashSet::new();
        let mut misplaced = Vec::new();
        for line in 1..=structure.line_count() {
            if structure.kind(line) != LineKind::Statement {
                continue;
            }
            let (Some(width), Some(first)) =
                (indent_width(source, line), structure.first_token(line))
            else {
                continue;
            };
            let block = blocks
                .entry(structure.innermost(line))
                .or_insert_with(|| BlockIndent {
                    base: width,
                    group: Vec::new(),
                });
            if width == block.base {
                block.group.clear();
                continue;
            }
            if width > block.base && aligned_with_previous_line(source, &structure, line, &aligned)
            {
                aligned.insert(line);
                continue;
            }
            if allow_groups
                && group_width > 0
                && width == block.base + group_width
                && structure.is_statement_start(line)
            {
                block.group.push((line, first));
                continue;
            }
            let target = if width > block.base && !block.group.is_empty() {
                block.base + group_width
            } else {
                block.base
            };
            misplaced.push((line, first, target));
        }
        for block in blocks.into_values() {
            misplaced.extend(
                block
                    .group
                    .into_iter()
                    .map(|(line, first)| (line, first, block.base)),
            );
        }
        misplaced.sort_by_key(|&(line, _, _)| line);
        for (line, first, target) in misplaced {
            ctx.add_offense_with_fix(
                indentation_range(source, line, first),
                "Inconsistent indentation detected.",
                structure.reindent(source, line, target),
            );
        }
    }
}

/// Whether the first token of line `n` starts in the same column as a token preceded by
/// whitespace on the nearest code line above, as in column-aligned tables. The first token of that
/// line only counts when the line was itself accepted as aligned.
fn aligned_with_previous_line(
    source: &Source,
    structure: &BlockStructure,
    n: u32,
    aligned: &HashSet<u32>,
) -> bool {
    let Some(first) = structure.first_token(n) else {
        return false;
    };
    let Some(previous) = (1..n).rev().find(|&line| structure.has_code(line)) else {
        return false;
    };
    let tokens: Vec<_> = source.code_tokens_on_line(previous).collect();
    tokens.iter().enumerate().any(|(index, token)| {
        let spaced = match index.checked_sub(1) {
            Some(before) => tokens[before].end < token.start,
            None => aligned.contains(&previous),
        };
        spaced && token.col == first.col
    })
}

inventory::submit! { Registration(|| Box::new(IndentationConsistency)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/IndentationConsistency";
    const MESSAGE: &str = "Inconsistent indentation detected.";

    #[test]
    fn accepts_consistent_blocks() {
        expect_no_offenses(
            READER,
            "local a = 1\nlocal b = 2\n\nfunction f()\n  x()\n\n  if a then\n    y()\n  else\n    z()\n  end\n  return w\nend\n",
        );
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_inconsistent_statements() {
        expect_offense(
            READER,
            "function f()\n  x()\n     y()\n  z()\nend\n",
            3,
            1,
            MESSAGE,
        );
        expect_offense(READER, "a()\n b()\n", 2, 1, MESSAGE);
        expect_offense(
            READER,
            "if x then\n  a()\nelse\n  b()\n c()\nend\n",
            5,
            1,
            MESSAGE,
        );
    }

    #[test]
    fn flags_inconsistent_statements_without_groups() {
        let options = "AllowIndentedGroups: false";
        let offenses = inspect_with(
            READER,
            "function f()\n  x()\n    y()\n  z()\nend\n",
            options,
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (3, 1));
        let offenses = inspect_with(
            READER,
            "function f()\n  local x = 1\n    x.a = 2\n    x.b = 3\n  return x\nend\n",
            options,
        );
        let lines: Vec<u32> = offenses.iter().map(|offense| offense.line).collect();
        assert_eq!(lines, vec![3, 4]);
    }

    #[test]
    fn checks_items_of_line_ending_brackets() {
        expect_no_offenses(READER, "local t = {\n  'a', 'b',\n  'c',\n  d = 1\n}\n");
        expect_offense(READER, "local t = {\n  'a',\n   'b'\n}\n", 3, 1, MESSAGE);
        expect_no_offenses(
            READER,
            "local t = {\n  ['a'] = 1,  ['b'] = 2,\n              ['c'] = 3,\n              ['d'] = 4\n}\n",
        );
        expect_offense(
            READER,
            "local t = {\n  ['a'] = 1,\n     ['c'] = 3\n}\n",
            3,
            1,
            MESSAGE,
        );
    }

    #[test]
    fn ignores_continuations_and_closers() {
        expect_no_offenses(
            READER,
            "function f()\n  local s = str\n               :gsub('a', 'b')\n  local x = a and\n        b\n  foo(a,\n      b)\n    end\n",
        );
        expect_no_offenses(READER, "local t = { a,\n    b,\n  c }\n");
    }

    #[test]
    fn ignores_multiline_strings_comments_and_tabs() {
        expect_no_offenses(READER, "local s = [[\n  a\n b]]\n-- c\n   -- d\nx()\n");
        expect_no_offenses(READER, "a()\n\tb()\n");
    }

    #[test]
    fn statements_on_the_opener_line_do_not_set_the_reference() {
        expect_no_offenses(READER, "if x then a()\n  b()\n  c()\nend\n");
    }

    #[test]
    fn allows_groups_one_width_deeper_that_return_to_the_base() {
        expect_no_offenses(
            READER,
            "function f()\n  net.Start(id)\n    net.WriteString(s)\n\n    net.WriteUInt(1, 8)\n  net.Send(t)\n\n  \
             local panel = vgui.Create('DPanel')\n    panel:Dock(FILL)\n  return panel\nend\n",
        );
        expect_no_offenses(
            READER,
            "cam.Start3D2D(pos, ang, 0.1)\n  draw.text(a)\n  if x then\n    y()\n  end\ncam.End3D2D()\n",
        );
    }

    #[test]
    fn flags_groups_that_never_return_to_the_base() {
        expect_offenses(
            READER,
            "function f()\n  local x = 1\n    x.a = 2\n    x.b = 3\nend\n",
            &[(3, 1, MESSAGE), (4, 1, MESSAGE)],
        );
        expect_offenses(
            READER,
            "local t = {\n  a,\n  b,\n    c,\n    d\n}\n",
            &[(4, 1, MESSAGE), (5, 1, MESSAGE)],
        );
    }

    #[test]
    fn items_of_brackets_never_form_groups() {
        expect_offense(
            READER,
            "local t = {\n  abc = 1,\n    b = 2,\n  c = 3\n}\n",
            3,
            1,
            MESSAGE,
        );
    }

    #[test]
    fn flags_groups_of_another_depth() {
        expect_offense(
            READER,
            "function f()\n  a()\n   b()\n  c()\nend\n",
            3,
            1,
            MESSAGE,
        );
        expect_offenses(
            READER,
            "function f()\n  a()\n    b()\n      c()\n    d()\n  e()\nend\n",
            &[(4, 1, MESSAGE)],
        );
        expect_offense(
            READER,
            "function f()\n  a()\n      b()\n  c()\nend\n",
            3,
            1,
            MESSAGE,
        );
    }

    #[test]
    fn corrects_lines_inside_a_group_to_the_group_level() {
        expect_correction(
            READER,
            "function f()\n  a()\n    b()\n      c()\n    d()\n  e()\n   g()\nend\n",
            "function f()\n  a()\n    b()\n    c()\n    d()\n  e()\n  g()\nend\n",
        );
    }

    #[test]
    fn group_width_follows_the_indentation_width_option() {
        let src = "function f()\n    a()\n        b()\n    c()\nend\n";
        assert_eq!(inspect(READER, src).len(), 1);
        assert!(inspect_with(READER, src, "IndentationWidth: 4").is_empty());
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "function f()\n  x()\n    local y = a and\n      b\n z()\nend\n",
            "function f()\n  x()\n  local y = a and\n    b\n  z()\nend\n",
        );
    }
}
