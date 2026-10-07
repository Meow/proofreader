//! `Layout/EmptyLineAfterGuardClause`.

use super::block_structure::{BlockStructure, LineKind, insert_blank_lines_after};
use crate::reader::{Context, Reader, Registration};

/// Flags a one-line guard clause (`if ... then return|continue|break ... end`) that is not
/// followed by a blank line, a closer or another guard clause.
pub struct EmptyLineAfterGuardClause;

impl Reader for EmptyLineAfterGuardClause {
    fn name(&self) -> &'static str {
        "Layout/EmptyLineAfterGuardClause"
    }

    fn description(&self) -> &'static str {
        "Checks for an empty line after a guard clause."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let structure = BlockStructure::new(source);
        for line in 1..structure.line_count() {
            let Some(guard) = structure.guard_clause(source, line) else {
                continue;
            };
            let next = line + 1;
            if source.is_blank(next)
                || matches!(structure.kind(next), LineKind::Closer | LineKind::Skipped)
                || structure.guard_clause(source, next).is_some()
            {
                continue;
            }
            let block = structure.block(guard);
            let (Some(closer), Some(edit)) =
                (block.closer, insert_blank_lines_after(source, line, 1))
            else {
                continue;
            };
            ctx.add_offense_with_fix(
                block.opener.start..closer.end,
                "Add empty line after guard clause.",
                vec![edit],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(EmptyLineAfterGuardClause)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EmptyLineAfterGuardClause";
    const MESSAGE: &str = "Add empty line after guard clause.";

    #[test]
    fn accepts_guards_followed_by_blank_lines_closers_or_guards() {
        expect_no_offenses(
            READER,
            "function f(a)\n  if !a then return end\n  if a.b then return false end\n\n  x()\n  if y then return end\nend\n",
        );
        expect_no_offenses(
            READER,
            "for _, v in ipairs(t) do\n  if !v then continue end\nend\n",
        );
        expect_no_offenses(READER, "if !a then return end");
        expect_no_offenses(READER, "if !a then return end\n\n\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_guards_followed_by_code() {
        expect_offense(READER, "if !a then return end\nx()\n", 1, 1, MESSAGE);
        expect_offense(
            READER,
            "while true do\n  if x then break end\n  y()\nend\n",
            2,
            3,
            MESSAGE,
        );
        expect_offense(
            READER,
            "if !a then return end\n-- note\nx()\n",
            1,
            1,
            MESSAGE,
        );
        let offenses = inspect(READER, "if !a then return end\nx()\n");
        assert_eq!(offenses[0].range, 0..21);
    }

    #[test]
    fn ignores_other_one_line_ifs() {
        expect_no_offenses(READER, "if a then b() end\nx()\n");
        expect_no_offenses(READER, "if a then return else b() end\nx()\n");
        expect_no_offenses(READER, "if a then\n  return\nend\nx()\n");
        expect_no_offenses(READER, "x() if a then return end\ny()\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "function f(a)\n  if !a then return end\n  x()\nend\n",
            "function f(a)\n  if !a then return end\n\n  x()\nend\n",
        );
    }
}
