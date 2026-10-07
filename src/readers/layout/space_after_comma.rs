//! `Layout/SpaceAfterComma`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::gap_after;
use crate::token::TokenKind;

/// Flags a comma directly followed by code on the same line.
///
/// A comma before a closing `)`, `]` or `}` or before a comment is left to other readers, and
/// extra spaces after a comma are left to `Layout/ExtraSpacing`.
pub struct SpaceAfterComma;

impl Reader for SpaceAfterComma {
    fn name(&self) -> &'static str {
        "Layout/SpaceAfterComma"
    }

    fn description(&self) -> &'static str {
        "Checks for a missing space after a comma."
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
            let Some(gap) = gap_after(source, code_index) else {
                continue;
            };
            if !gap.is_empty()
                || gap.after.kind.is_comment()
                || matches!(
                    gap.after.kind,
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                )
            {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.before.range(),
                "Space missing after comma.",
                vec![Edit::insert(gap.before.end, " ")],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceAfterComma)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceAfterComma";

    #[test]
    fn flags_missing_space() {
        expect_offense(READER, "f(a,b)\n", 1, 4, "Space missing after comma.");
        expect_offenses(
            READER,
            "local a,b = 1,2\n",
            &[
                (1, 8, "Space missing after comma."),
                (1, 14, "Space missing after comma."),
            ],
        );
        expect_offense(
            READER,
            "t = { 1,'x' }\n",
            1,
            8,
            "Space missing after comma.",
        );
    }

    #[test]
    fn accepts_spaced_commas() {
        expect_no_offenses(READER, "f(a, b)\nt = {\n  1,\n  2\n}\n");
        expect_no_offenses(READER, "f(a,  b)\nf(a,\tb)\n");
        expect_no_offenses(READER, "s = 'a,b'\n-- a,b\nx = [[a,b]]\n");
        expect_no_offenses(READER, "t = { 1,}\nf(a,--[[x]] b)\nf(a,\n  b)\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "f(a,b,c)\n", "f(a, b, c)\n");
        expect_correction(
            READER,
            "for k,v in pairs(t) do end\n",
            "for k, v in pairs(t) do end\n",
        );
    }
}
