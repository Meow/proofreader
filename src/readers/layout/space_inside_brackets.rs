//! `Layout/SpaceInsideBrackets`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::spacing::{Side, inside_gaps, spaced_gap_offense};
use crate::source::Source;
use crate::token::TokenKind;

/// Checks the whitespace directly inside `[` and `]` of indexing and table keys on the same
/// line.
///
/// With `EnforcedStyle: no_space` (the default) there is none; with `space` there is exactly one
/// (`t[ 1 ]`, `{ [ 'a' ] = 1 }`); with `space_when_nested` (the Helix style) there is exactly one
/// inside a pair whose contents hold another pair of brackets (`a[ b[i] ]`) and none otherwise. Long strings and comments are single tokens and never
/// affected. In `no_space` style the space in `t[ [[key]] ]` is needed to keep `[[[` from
/// opening a long string, so it is accepted.
pub struct SpaceInsideBrackets;

impl Reader for SpaceInsideBrackets {
    fn name(&self) -> &'static str {
        "Layout/SpaceInsideBrackets"
    }

    fn description(&self) -> &'static str {
        "Checks for spaces inside square brackets."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("no_space".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let style = ctx.option_str("EnforcedStyle", "no_space");
        let nested = if style == "space_when_nested" {
            nested_pairs(source)
        } else {
            Vec::new()
        };
        for (side, gap) in inside_gaps(source, TokenKind::LBracket, TokenKind::RBracket) {
            let bracket = match side {
                Side::Open | Side::Empty => gap.before.start,
                Side::Close => gap.after.start,
            };
            let spaced = style == "space" || nested.binary_search(&bracket).is_ok();
            if spaced {
                if let Some((range, message, edit)) =
                    spaced_gap_offense(source, side, gap, "square brackets")
                {
                    ctx.add_offense_with_fix(range, message, vec![edit]);
                }
                continue;
            }
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

/// Start offsets of the `[` and `]` of every bracket pair whose contents hold another `[`, sorted.
fn nested_pairs(source: &Source) -> Vec<usize> {
    let mut open: Vec<(usize, bool)> = Vec::new();
    let mut found = Vec::new();
    for token in source.code_tokens() {
        match token.kind {
            TokenKind::LBracket => {
                if let Some(outer) = open.last_mut() {
                    outer.1 = true;
                }
                open.push((token.start, false));
            }
            TokenKind::RBracket => {
                if let Some((start, holds)) = open.pop()
                    && holds
                {
                    found.push(start);
                    found.push(token.start);
                }
            }
            _ => {}
        }
    }
    found.sort_unstable();
    found
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
    fn space_when_nested() {
        let options = "EnforcedStyle: space_when_nested";
        assert!(
            inspect_with(
                READER,
                "x = a[ b[i] ]\ny = a[i]\nz = { [ t[1] ] = 1 }\n",
                options
            )
            .is_empty()
        );
        assert_eq!(inspect_with(READER, "x = a[b[ i ]]\n", options).len(), 4);
        assert_eq!(
            autocorrect_with(READER, "x = a[b[ i ]]\ny = c[ j ]\n", options),
            "x = a[ b[i] ]\ny = c[j]\n"
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "x = t[ a[ 1 ] ]\n", "x = t[a[1]]\n");
        expect_correction(READER, "x = t[ [[k]] ]\n", "x = t[ [[k]]]\n");
    }

    #[test]
    fn space_style() {
        let options = "EnforcedStyle: space";
        assert!(
            inspect_with(
                READER,
                "x = t[ 1 ]\nt = { [ 'a' ] = 1 }\nx = t[ [[k]] ]\n",
                options
            )
            .is_empty()
        );
        let offenses = inspect_with(READER, "x = t[1  ]\n", options);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|o| (o.line, o.col, o.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 6, "Space inside square brackets missing."),
                (1, 8, "Extra space inside square brackets detected."),
            ]
        );
        assert_eq!(
            autocorrect_with(READER, "x = t[a[1]]\nx = t[ [[k]]]\n", options),
            "x = t[ a[ 1 ] ]\nx = t[ [[k]] ]\n"
        );
    }
}
