//! `Layout/SpaceInsideBraces`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{Side, inside_gaps};
use crate::token::TokenKind;

/// Checks the whitespace directly inside the braces of a table constructor on the same line.
///
/// With `EnforcedStyle: space` (the default) a non-empty table needs exactly one space after `{`
/// and before `}`; with `no_space` it needs none. Empty tables are written `{}` in both styles.
/// A `{` ending a line or a `}` starting one is not checked.
pub struct SpaceInsideBraces;

impl Reader for SpaceInsideBraces {
    fn name(&self) -> &'static str {
        "Layout/SpaceInsideBraces"
    }

    fn description(&self) -> &'static str {
        "Checks the spacing inside table constructor braces."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("space".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let spaced = ctx.option_str("EnforcedStyle", "space") != "no_space";
        for (side, gap) in inside_gaps(source, TokenKind::LBrace, TokenKind::RBrace) {
            let brace = match side {
                Side::Open | Side::Empty => gap.before,
                Side::Close => gap.after,
            };
            let text = source.text_of(brace);
            let (message, edit) = if side == Side::Empty {
                if gap.is_empty() {
                    continue;
                }
                (
                    "Space inside empty braces detected.".to_owned(),
                    Edit::remove(gap.range()),
                )
            } else if !spaced {
                if gap.is_empty() {
                    continue;
                }
                (
                    format!("Space inside {text} detected."),
                    Edit::remove(gap.range()),
                )
            } else if gap.is_empty() {
                (
                    format!("Space inside {text} missing."),
                    Edit::insert(gap.range().start, " "),
                )
            } else if !gap.is_single_space(source) {
                (
                    format!("Extra space inside {text} detected."),
                    Edit::replace(gap.range(), " "),
                )
            } else {
                continue;
            };
            let range = if gap.is_empty() {
                brace.range()
            } else {
                gap.range()
            };
            ctx.add_offense_with_fix(range, message, vec![edit]);
        }
    }
}

inventory::submit! { Registration(|| Box::new(SpaceInsideBraces)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceInsideBraces";

    #[test]
    fn flags_missing_spaces() {
        expect_offenses(
            READER,
            "t = {a}\n",
            &[
                (1, 5, "Space inside { missing."),
                (1, 7, "Space inside } missing."),
            ],
        );
        expect_offense(READER, "t = { a = 1}\n", 1, 12, "Space inside } missing.");
    }

    #[test]
    fn flags_extra_spaces() {
        expect_offense(
            READER,
            "t = {  a }\n",
            1,
            6,
            "Extra space inside { detected.",
        );
        expect_offense(
            READER,
            "t = { a\t}\n",
            1,
            8,
            "Extra space inside } detected.",
        );
        expect_offense(
            READER,
            "t = { }\n",
            1,
            6,
            "Space inside empty braces detected.",
        );
    }

    #[test]
    fn accepts_spaced_and_multiline_tables() {
        expect_no_offenses(READER, "t = { a = 1, 'x' }\nt = {}\nf{ 1 }\n");
        expect_no_offenses(READER, "t = {\n  a = 1\n}\nt = { a,\n  b }\n");
        expect_no_offenses(READER, "t = { -- note\n  a }\ns = '{a}'\n-- {a}\n");
    }

    #[test]
    fn no_space_style() {
        let options = "EnforcedStyle: no_space";
        assert!(inspect_with(READER, "t = {a}\nt = {}\n", options).is_empty());
        let offenses = inspect_with(READER, "t = { a }\nt = { }\n", options);
        let messages: Vec<&str> = offenses.iter().map(|o| o.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                "Space inside { detected.",
                "Space inside } detected.",
                "Space inside empty braces detected."
            ]
        );
        assert_eq!(
            autocorrect_with(READER, "t = {  a }\n", options),
            "t = {a}\n"
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "t = {a, {b}}\n", "t = { a, { b } }\n");
        expect_correction(READER, "t = {   a  }\nt = {  }\n", "t = { a }\nt = {}\n");
    }
}
