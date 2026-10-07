//! `Layout/EmptyLineBetweenDefs`.

use yaml_rust2::Yaml;

use super::block_structure::{
    BlockStructure, LineKind, insert_blank_lines_after, last_code_token_on_line,
};
use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags consecutive function definitions not separated by exactly `NumberOfEmptyLines` blank
/// lines, counted from the closing `end` to the next definition or its doc comment.
///
/// With `AllowAdjacentOneLineDefs`, two adjacent one-line definitions need no blank line.
pub struct EmptyLineBetweenDefs;

impl Reader for EmptyLineBetweenDefs {
    fn name(&self) -> &'static str {
        "Layout/EmptyLineBetweenDefs"
    }

    fn description(&self) -> &'static str {
        "Checks for empty lines between function definitions."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("NumberOfEmptyLines", Yaml::Integer(1)),
            ("AllowAdjacentOneLineDefs", Yaml::Boolean(true)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let wanted = ctx.option_usize("NumberOfEmptyLines", 1);
        let allow_one_line = ctx.option_bool("AllowAdjacentOneLineDefs", true);
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        for line in 1..=structure.line_count() {
            let Some(definition) = structure.definition(source, line) else {
                continue;
            };
            let block = structure.block(definition);
            let Some(end) = block.closer.filter(|closer| closer.kind == TokenKind::End) else {
                continue;
            };
            if last_code_token_on_line(source, end.line) != Some(end) {
                continue;
            }
            let Some((start, next_definition)) = next_definition(source, &structure, end.line)
            else {
                continue;
            };
            if allow_one_line
                && block.is_one_line()
                && structure.block(next_definition).is_one_line()
            {
                continue;
            }
            let found = (start - end.line - 1) as usize;
            if found == wanted {
                continue;
            }
            let fix = if found < wanted {
                insert_blank_lines_after(source, end.line, wanted - found).map(|edit| vec![edit])
            } else {
                let first_removed = end.line + 1 + u32::try_from(wanted).unwrap_or(u32::MAX);
                Some(vec![Edit::remove(
                    source.line_range(first_removed).start..source.line_range(start).start,
                )])
            };
            let Some(lead) = source
                .tokens_on_line(start)
                .iter()
                .find(|token| !token.is_trivia())
            else {
                continue;
            };
            let plural = if wanted == 1 { "" } else { "s" };
            let message = format!(
                "Expected {wanted} empty line{plural} between function definitions; found {found}."
            );
            match fix {
                Some(edits) => ctx.add_offense_with_fix(lead.range(), message, edits),
                None => ctx.add_offense(lead.range(), message),
            };
        }
    }
}

/// The start line (doc comment included) and block of the definition following the line `end`,
/// when only blank lines separate them.
fn next_definition(source: &Source, structure: &BlockStructure, end: u32) -> Option<(u32, usize)> {
    let start = (end + 1..=structure.line_count()).find(|&line| !source.is_blank(line))?;
    if let Some(definition) = structure.definition(source, start) {
        return Some((start, definition));
    }
    if structure.kind(start) != LineKind::Comment {
        return None;
    }
    let line =
        (start..=structure.line_count()).find(|&line| structure.kind(line) != LineKind::Comment)?;
    let definition = structure.definition(source, line)?;
    (structure.doc_comment_start(source, line) == Some(start)).then_some((start, definition))
}

inventory::submit! { Registration(|| Box::new(EmptyLineBetweenDefs)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLineBetweenDefs";

    #[test]
    fn accepts_separated_definitions() {
        expect_no_offenses(
            READER,
            "function a()\nend\n\n--- Doc.\n-- @return [Number]\nfunction b()\n  return 1\nend\n\nlocal function c()\nend\n",
        );
        expect_no_offenses(
            READER,
            "function a() return 1 end\nfunction b() return 2 end\n",
        );
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_missing_blank_lines() {
        expect_offense(
            READER,
            "function a()\nend\nfunction b()\nend\n",
            3,
            1,
            "Expected 1 empty line between function definitions; found 0.",
        );
        expect_offense(
            READER,
            "function a()\nend\n--- Doc.\nlocal function b()\nend\n",
            3,
            1,
            "Expected 1 empty line between function definitions; found 0.",
        );
    }

    #[test]
    fn flags_extra_blank_lines() {
        expect_offense(
            READER,
            "function a()\nend\n\n\nfunction b()\nend\n",
            5,
            1,
            "Expected 1 empty line between function definitions; found 2.",
        );
    }

    #[test]
    fn ignores_non_adjacent_definitions() {
        expect_no_offenses(READER, "function a()\nend\nx()\nfunction b()\nend\n");
        expect_no_offenses(READER, "function a()\nend\n-- plain\nfunction b()\nend\n");
        expect_no_offenses(READER, "local f = function()\nend\nfunction b()\nend\n");
        expect_no_offenses(READER, "function a()\nend)\nfunction b()\nend\n");
    }

    #[test]
    fn honours_options() {
        let src = "function a() return 1 end\nfunction b() return 2 end\n";
        assert_eq!(
            inspect_with(READER, src, "AllowAdjacentOneLineDefs: false").len(),
            1
        );
        let offenses = inspect_with(
            READER,
            "function a()\nend\n\nfunction b()\nend\n",
            "NumberOfEmptyLines: 2",
        );
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Expected 2 empty lines between function definitions; found 1."
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "function a()\nend\n\nfunction b()\nend\n",
                "NumberOfEmptyLines: 2"
            ),
            "function a()\nend\n\n\nfunction b()\nend\n"
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "function a()\nend\nfunction b()\nend\n",
            "function a()\nend\n\nfunction b()\nend\n",
        );
        expect_correction(
            READER,
            "function a()\nend\n\n  \n\n--- Doc.\nfunction b()\nend\n",
            "function a()\nend\n\n--- Doc.\nfunction b()\nend\n",
        );
    }
}
