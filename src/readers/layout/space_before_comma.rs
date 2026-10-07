//! `Layout/SpaceBeforeComma`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::gap_before;
use crate::token::TokenKind;

/// Flags whitespace between code and a following comma on the same line.
pub struct SpaceBeforeComma;

impl Reader for SpaceBeforeComma {
    fn name(&self) -> &'static str {
        "Layout/SpaceBeforeComma"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces before a comma."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for code_index in 0..source.code_token_indexes().len() {
            if source
                .code_token(code_index)
                .is_none_or(|token| token.kind != TokenKind::Comma)
            {
                continue;
            }
            let Some(gap) = gap_before(source, code_index) else {
                continue;
            };
            if gap.is_empty() || gap.before.kind.is_comment() {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Space found before comma.",
                vec![Edit::remove(gap.range())],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceBeforeComma)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceBeforeComma";

    #[test]
    fn flags_space_before_comma() {
        expect_offense(READER, "f(a , b)\n", 1, 4, "Space found before comma.");
        expect_offense(
            READER,
            "local a\t, b = 1, 2\n",
            1,
            8,
            "Space found before comma.",
        );
        expect_offense(
            READER,
            "t = { 1  , 2 }\n",
            1,
            8,
            "Space found before comma.",
        );
    }

    #[test]
    fn accepts_clean_commas() {
        expect_no_offenses(READER, "f(a, b)\nt = {\n  1\n  , 2\n}\n");
        expect_no_offenses(READER, "s = 'a , b'\n-- a , b\nf(a --[[x]], b)\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "f(a , b ,c)\n", "f(a, b,c)\n");
    }
}
