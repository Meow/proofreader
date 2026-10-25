//! `Layout/SpaceInsideParens`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{inside_gaps, spaced_gap_offense};
use crate::token::TokenKind;

/// Checks the whitespace directly inside `(` and `)` on the same line.
///
/// With `EnforcedStyle: no_space` (the default) there is none, `( )` included; with `space`
/// non-empty parentheses need exactly one space after `(` and before `)` (`f( a, b )`) and
/// empty ones none (`f()`). A `(` ending a line, a `)` starting one and gaps next to a comment
/// are not checked.
pub struct SpaceInsideParens;

impl Reader for SpaceInsideParens {
    fn name(&self) -> &'static str {
        "Layout/SpaceInsideParens"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces inside parentheses."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("no_space".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let spaced = ctx.option_str("EnforcedStyle", "no_space") == "space";
        for (side, gap) in inside_gaps(source, TokenKind::LParen, TokenKind::RParen) {
            if spaced {
                if let Some((range, message, edit)) =
                    spaced_gap_offense(source, side, gap, "parentheses")
                {
                    ctx.add_offense_with_fix(range, message, vec![edit]);
                }
                continue;
            }
            if gap.is_empty() {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                "Space inside parentheses detected.",
                vec![Edit::remove(gap.range())],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceInsideParens)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceInsideParens";
    const MESSAGE: &str = "Space inside parentheses detected.";

    #[test]
    fn flags_spaces_inside_parens() {
        expect_offense(READER, "f( a)\n", 1, 3, MESSAGE);
        expect_offense(READER, "f(a  )\n", 1, 4, MESSAGE);
        expect_offense(READER, "f( )\n", 1, 3, MESSAGE);
        expect_offenses(
            READER,
            "x = ( a + b )\n",
            &[(1, 6, MESSAGE), (1, 12, MESSAGE)],
        );
    }

    #[test]
    fn accepts_tight_parens_and_line_breaks() {
        expect_no_offenses(READER, "f(a, (b))\nf()\nf(\n  a\n)\nf(a,\n  b\n  )\n");
        expect_no_offenses(READER, "f( -- note\n  a)\ns = '( a )'\n-- ( a )\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "f( a, ( b ) )\nf( )\n", "f(a, (b))\nf()\n");
    }

    #[test]
    fn space_style() {
        let options = "EnforcedStyle: space";
        assert!(inspect_with(READER, "f( a, ( b ) )\nf()\nf(\n  a\n)\n", options).is_empty());
        let offenses = inspect_with(READER, "f(a  )\nf( )\n", options);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|o| (o.line, o.col, o.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 2, "Space inside parentheses missing."),
                (1, 4, "Extra space inside parentheses detected."),
                (2, 3, "Space inside empty parentheses detected."),
            ]
        );
        assert_eq!(
            autocorrect_with(READER, "f(a, (b))\nf(  )\nx = (y)\n", options),
            "f( a, ( b ) )\nf()\nx = ( y )\n"
        );
        assert_eq!(
            autocorrect_with(READER, "f( a, ( b ) )\n", options),
            "f( a, ( b ) )\n"
        );
    }
}
