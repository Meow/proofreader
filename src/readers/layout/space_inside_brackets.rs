//! `Layout/SpaceInsideBrackets`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{Side, inside_gaps};
use crate::token::TokenKind;

/// Flags whitespace directly inside `[` and `]` of indexing and table keys on the same line.
///
/// Long strings and comments are single tokens and never affected. The space in `t[ [[key]] ]`
/// is needed to keep `[[[` from opening a long string, so it is accepted.
pub struct SpaceInsideBrackets;

impl Reader for SpaceInsideBrackets {
    fn name(&self) -> &'static str {
        "Layout/SpaceInsideBrackets"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces inside square brackets."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for (side, gap) in inside_gaps(source, TokenKind::LBracket, TokenKind::RBracket) {
            if gap.is_empty()
                || (side == Side::Open && source.text_of(gap.after).starts_with(['[', '=']))
            {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Space inside square brackets detected.",
                vec![Edit::remove(gap.range())],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceInsideBrackets)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceInsideBrackets";
    const MESSAGE: &str = "Space inside square brackets detected.";

    #[test]
    fn flags_spaces_inside_brackets() {
        expect_offense(READER, "x = t[ 1]\n", 1, 7, MESSAGE);
        expect_offense(READER, "x = t[1 ]\n", 1, 8, MESSAGE);
        expect_offenses(
            READER,
            "t = { [ 'a' ] = 1 }\n",
            &[(1, 8, MESSAGE), (1, 12, MESSAGE)],
        );
    }

    #[test]
    fn accepts_tight_brackets_and_long_strings() {
        expect_no_offenses(
            READER,
            "x = t[1]\nt = { ['a'] = 1 }\ns = [[ a ]]\n--[[ a ]]\n",
        );
        expect_no_offenses(READER, "x = t[\n  1\n]\ns = '[ a ]'\n");
        expect_no_offenses(READER, "x = t[ [[k]]]\nx = t[ [==[k]==]]\n");
        expect_offense(READER, "x = t[ [[k]] ]\n", 1, 13, MESSAGE);
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "x = t[ a[ 1 ] ]\n", "x = t[a[1]]\n");
        expect_correction(READER, "x = t[ [[k]] ]\n", "x = t[ [[k]]]\n");
    }
}
