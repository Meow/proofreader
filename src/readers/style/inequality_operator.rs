//! `Style/InequalityOperator`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags the Lua inequality operator `~=` in favour of GLua's `!=`.
pub struct InequalityOperator;

impl Reader for InequalityOperator {
    fn name(&self) -> &'static str {
        "Style/InequalityOperator"
    }

    fn description(&self) -> &'static str {
        "Checks for uses of `~=` instead of `!=`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for token in source.code_tokens() {
            if token.kind == TokenKind::Ne && source.text_of(token) == "~=" {
                ctx.add_offense_with_fix(
                    token.range(),
                    "Use `!=` instead of `~=`.",
                    vec![Edit::replace(token.range(), "!=")],
                );
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(InequalityOperator)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/InequalityOperator";

    #[test]
    fn flags_tilde_equals() {
        expect_offense(
            READER,
            "if a ~= b then end\n",
            1,
            6,
            "Use `!=` instead of `~=`.",
        );
        expect_correction(READER, "if a ~= b then end\n", "if a != b then end\n");
        expect_correction(READER, "x = a~=b\n", "x = a!=b\n");
    }

    #[test]
    fn accepts_other_code() {
        expect_no_offenses(READER, "if a != b then end\n");
        expect_no_offenses(READER, "s = '~='\nt = { ['~='] = 1 }\n-- a ~= b\n");
    }
}
