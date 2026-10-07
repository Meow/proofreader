//! `Layout/ExtraSpacing`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::alignment::{
    aligned_with_adjacent_line, assignment_groups, char_column,
};
use crate::readers::layout::spacing::{Gap, gap_before, is_binary_operator, is_callee_end};
use crate::source::Source;
use crate::token::TokenKind;

/// Flags runs of more than one space (or any tab) between two code tokens on a line.
///
/// Leading indentation and trailing whitespace are left to other readers, and so are the gaps
/// other spacing readers own: around binary operators and `=` (`Layout/SpaceAroundOperators`),
/// directly inside brackets, braces and parentheses (`Layout/SpaceInside*`), before a comma
/// (`Layout/SpaceBeforeComma`), before a call's `(` (`Layout/SpaceBeforeParen`) and after `!`
/// (`Layout/SpaceAfterNot`). With `AllowForAlignment`, a run is accepted when the token after it
/// starts at the same column as a token on an adjacent line. With `ForceEqualSignAlignment`, the
/// `=` of directly consecutive assignment lines with the same indentation must line up; groups
/// that do not are aligned at the leftmost column that keeps a space before every `=`.
pub struct ExtraSpacing;

impl Reader for ExtraSpacing {
    fn name(&self) -> &'static str {
        "Layout/ExtraSpacing"
    }

    fn description(&self) -> &'static str {
        "Checks for extra spaces between tokens."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("AllowForAlignment", Yaml::Boolean(true)),
            ("ForceEqualSignAlignment", Yaml::Boolean(false)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let allow_alignment = ctx.option_bool("AllowForAlignment", true);
        for code_index in 1..source.code_token_indexes().len() {
            let Some(gap) = gap_before(source, code_index) else {
                continue;
            };
            let text = &source.text[gap.range()];
            if text.len() < 2 && !text.contains('\t') {
                continue;
            }
            if owned_elsewhere(source, code_index, gap) {
                continue;
            }
            if allow_alignment && aligned_with_adjacent_line(source, gap.after, |_| true) {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Unnecessary spacing detected.",
                vec![Edit::replace(gap.range(), " ")],
            );
        }
        if ctx.option_bool("ForceEqualSignAlignment", false) {
            force_equal_sign_alignment(ctx);
        }
    }
}

/// Whether another spacing reader is responsible for `gap`, which lies before code index
/// `code_index`.
fn owned_elsewhere(source: &Source, code_index: usize, gap: Gap) -> bool {
    let before = gap.before.kind;
    let after = gap.after.kind;
    if before.is_comment() || after.is_comment() {
        return false;
    }
    is_binary_operator(source, code_index)
        || is_binary_operator(source, code_index - 1)
        || matches!(
            before,
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace | TokenKind::Bang
        )
        || matches!(
            after,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace | TokenKind::Comma
        )
        || (after == TokenKind::LParen && is_callee_end(before))
}

/// Reports the `=` signs of every assignment group that is not aligned yet, with fixes that move
/// them all to the leftmost column where every `=` of the group still has a space before it.
///
/// Aiming at that tight column (rather than the rightmost `=`) gives the same fixed point that
/// `Layout/SpaceAroundOperators` corrects towards, so the two never undo each other.
fn force_equal_sign_alignment(ctx: &mut Context) {
    let source = ctx.source;
    for group in assignment_groups(source) {
        let columns: Vec<usize> = group
            .iter()
            .map(|line| char_column(source, line.equals))
            .collect();
        if columns.windows(2).all(|pair| pair[0] == pair[1]) {
            continue;
        }
        let Some(target) = group.iter().map(|line| line.minimum_column).max() else {
            continue;
        };
        for (line, column) in group.iter().zip(columns) {
            if column == target || line.before.kind.is_comment() {
                continue;
            }
            let padding = " ".repeat(target + 1 - line.minimum_column);
            ctx.add_offense_with_fix(
                line.equals.range(),
                "`=` is not aligned with the adjacent assignments.",
                vec![Edit::replace(line.before.end..line.equals.start, padding)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(ExtraSpacing)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/ExtraSpacing";
    const MESSAGE: &str = "Unnecessary spacing detected.";

    #[test]
    fn flags_runs_of_spaces() {
        expect_offense(READER, "local  x = 1\n", 1, 6, MESSAGE);
        expect_offense(READER, "f(a,  b)\n", 1, 5, MESSAGE);
        expect_offense(READER, "return  x\n", 1, 7, MESSAGE);
        expect_offense(READER, "x = 1  -- note\n", 1, 6, MESSAGE);
        expect_offense(READER, "if x then\treturn end\n", 1, 10, MESSAGE);
    }

    #[test]
    fn ignores_indentation_trailing_space_strings_and_comments() {
        expect_no_offenses(
            READER,
            "if x then\n    y()\nend  \nz = 'a   b'\n-- a   b\nw = [[\n  a   b\n]]\n",
        );
        expect_no_offenses(READER, "x = 1  ");
    }

    #[test]
    fn leaves_gaps_owned_by_other_readers() {
        expect_no_offenses(READER, "x  = 1\ny = a  + b\nz = a ..  b\n");
        expect_no_offenses(
            READER,
            "f(  a  )\nt = {  a  }\nt[  1  ] = 1\nf(a  , b)\nf  (a)\nx = !  y\n",
        );
    }

    #[test]
    fn allows_alignment() {
        expect_no_offenses(READER, "t = {\n  { 'foo', 1 },\n  { 'ba',  2 },\n}\n");
        expect_no_offenses(READER, "x = 1    -- one\nyy = 22  -- two\n");
        expect_no_offenses(
            READER,
            "  ['and']       = 'and',          ['in']        = 'in',\n  ['break']     = 'break',        ['local']     = 'local',\n",
        );
        expect_offense(READER, "x = 1    -- one\nyy = 22 -- two\n", 1, 6, MESSAGE);
        assert_eq!(
            inspect_with(
                READER,
                "x = 1    -- one\nyy = 22  -- two\n",
                "AllowForAlignment: false"
            )
            .len(),
            2
        );
    }

    #[test]
    fn force_equal_sign_alignment() {
        let options = "ForceEqualSignAlignment: true";
        let offenses = inspect_with(READER, "a = 1\nbcd = 2\n\nx = 1\n", options);
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "`=` is not aligned with the adjacent assignments."
        );
        assert_eq!((offenses[0].line, offenses[0].col), (1, 3));
        assert_eq!(
            autocorrect_with(
                READER,
                "local a = 1\nlocal bcd = 2\nt[1]   = 3\nf()\n",
                options
            ),
            "local a   = 1\nlocal bcd = 2\nt[1]      = 3\nf()\n"
        );
        assert!(inspect_with(READER, "a   = 1\nbcd = 2\n", options).is_empty());
        assert!(inspect_with(READER, "a     = 1\nbcd   = 2\n", options).is_empty());
        assert_eq!(
            autocorrect_with(READER, "a      = 1\nbcd = 2\n", options),
            "a   = 1\nbcd = 2\n"
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(
            READER,
            "local  x  =  {  a,   b }\n",
            "local x  =  {  a, b }\n",
        );
        expect_correction(READER, "return\t\tx\n", "return x\n");
    }
}
