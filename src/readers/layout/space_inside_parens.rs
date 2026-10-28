//! `Layout/SpaceInsideParens`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{Gap, Side, inside_gaps, spaced_gap_offense};
use crate::token::TokenKind;

/// Checks the whitespace directly inside `(` and `)` on the same line, and inside the `|` bars
/// around the parameters of a short function (`|a, b| -> a + b`), which follow the same style.
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
        let pairs = [
            (TokenKind::LParen, TokenKind::RParen, "parentheses"),
            (TokenKind::Bar, TokenKind::Bar, "parameter bars"),
        ];
        let mut gaps: Vec<(Side, Gap, &str)> = pairs
            .into_iter()
            .flat_map(|(open, close, noun)| {
                inside_gaps(source, open, close)
                    .into_iter()
                    .map(move |(side, gap)| (side, gap, noun))
            })
            .collect();
        gaps.sort_by_key(|(_, gap, _)| gap.before.start);
        for (side, gap, noun) in gaps {
            if spaced {
                if let Some((range, message, edit)) = spaced_gap_offense(source, side, gap, noun) {
                    ctx.add_offense_with_fix(range, message, vec![edit]);
                }
                continue;
            }
            if gap.is_empty() {
                continue;
            }
            ctx.add_offense_with_fix(
                gap.range(),
                format!("Space inside {noun} detected."),
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
    fn checks_short_function_bars() {
        let message = "Space inside parameter bars detected.";
        expect_offenses(
            READER,
            "f = | a, b | -> a\n",
            &[(1, 6, message), (1, 11, message)],
        );
        expect_correction(
            READER,
            "f = | a | -> | | -> ( a )\n",
            "f = |a| -> || -> (a)\n",
        );
        expect_no_offenses(READER, "f = |a, b| -> a | b\ng = |...| -> x | y\n");
        assert_eq!(
            autocorrect_with(READER, "f = |a| -> || -> 1\n", "EnforcedStyle: space"),
            "f = | a | -> || -> 1\n"
        );
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
