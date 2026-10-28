//! `Style/InequalityOperator`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Checks the spelling of the inequality operator: with `EnforcedStyle: bang` (the default) the
/// Lua operator `~=` is flagged in favor of GLua's `!=`, with `tilde` the other way round. A
/// `~=` after the target of an assignment statement (`flags ~= mask`) is xor assignment, not the
/// inequality operator, and is left alone.
pub struct InequalityOperator;

impl Reader for InequalityOperator {
    fn name(&self) -> &'static str {
        "Style/InequalityOperator"
    }

    fn description(&self) -> &'static str {
        "Checks that the inequality operator is spelled `!=` (or `~=`)."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("bang".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let (wanted, unwanted) = if ctx.option_str("EnforcedStyle", "bang") == "tilde" {
            ("~=", "!=")
        } else {
            ("!=", "~=")
        };
        let source = ctx.source;
        for token in source.code_tokens() {
            if token.kind == TokenKind::Ne && source.text_of(token) == unwanted {
                ctx.add_offense_with_fix(
                    token.range(),
                    format!("Use `{wanted}` instead of `{unwanted}`."),
                    vec![Edit::replace(token.range(), wanted)],
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

    #[test]
    fn leaves_xor_assignment_alone() {
        expect_no_offenses(READER, "flags ~= mask\nt.x ~= 1\n");
        expect_offense(READER, "x ~= a ~= b\n", 1, 8, "Use `!=` instead of `~=`.");
    }

    #[test]
    fn tilde_style() {
        let style = "EnforcedStyle: tilde";
        let offenses = inspect_with(READER, "if a != b and c ~= d then end\n", style);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 6));
        assert_eq!(offenses[0].message, "Use `~=` instead of `!=`.");
        assert_eq!(autocorrect_with(READER, "x = a!=b\n", style), "x = a~=b\n");
        assert!(inspect_with(READER, "s = '!='\n-- a != b\n", style).is_empty());
    }
}
