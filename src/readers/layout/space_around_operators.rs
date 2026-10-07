//! `Layout/SpaceAroundOperators`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::layout::alignment::is_aligned_operator;
use crate::readers::layout::spacing::{Gap, gap_after, gap_before, is_binary_operator};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Checks that binary operators and `=` have exactly one space on each side, and that `..` has
/// none (or one, with `ConcatStyle: space`).
///
/// Only sides where the neighbouring token is on the same line and is not a comment are checked.
/// With `AllowForAlignment`, extra spaces before an operator are accepted when the operator lines
/// up with the same operator on an adjacent line (blank and comment-only lines are looked past).
/// A space around `..` that keeps a number or a `.` from merging with the operator is never
/// reported.
pub struct SpaceAroundOperators;

impl Reader for SpaceAroundOperators {
    fn name(&self) -> &'static str {
        "Layout/SpaceAroundOperators"
    }

    fn description(&self) -> &'static str {
        "Checks that operators have a single space around them, and that `..` has none."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("ConcatStyle", Yaml::String("no_space".to_owned())),
            ("AllowForAlignment", Yaml::Boolean(true)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let concat_spaced = ctx.option_str("ConcatStyle", "no_space") == "space";
        let allow_alignment = ctx.option_bool("AllowForAlignment", true);
        for code_index in 0..source.code_token_indexes().len() {
            if !is_binary_operator(source, code_index) {
                continue;
            }
            let Some(operator) = source.code_token(code_index) else {
                continue;
            };
            let left = gap_before(source, code_index).filter(|gap| !gap.before.kind.is_comment());
            let right = gap_after(source, code_index).filter(|gap| !gap.after.kind.is_comment());
            let finding = if operator.kind == TokenKind::Concat && !concat_spaced {
                unspaced(source, operator, left, right)
            } else {
                spaced(source, operator, left, right, allow_alignment)
            };
            if let Some((message, edits)) = finding {
                ctx.add_offense_with_fix(operator.range(), message, edits);
            }
        }
    }
}

/// The message and fix for an operator that needs one space on each side, if it lacks them.
fn spaced(
    source: &Source,
    operator: &Token,
    left: Option<Gap>,
    right: Option<Gap>,
    allow_alignment: bool,
) -> Option<(String, Vec<Edit>)> {
    let aligned = allow_alignment && is_aligned_operator(source, operator);
    let mut missing = false;
    let mut edits = Vec::new();
    for (index, gap) in [left, right].into_iter().enumerate() {
        let Some(gap) = gap else {
            continue;
        };
        if gap.is_empty() {
            missing = true;
            edits.push(Edit::insert(gap.range().start, " "));
        } else if !gap.is_single_space(source) && !(index == 0 && aligned) {
            edits.push(Edit::replace(gap.range(), " "));
        }
    }
    if edits.is_empty() {
        return None;
    }
    let text = source.text_of(operator);
    let message = if missing {
        format!("Surrounding space missing for operator `{text}`.")
    } else {
        format!("Operator `{text}` should be surrounded by a single space.")
    };
    Some((message, edits))
}

/// The message and fix for a `..` that should have no spaces around it, if it has some.
fn unspaced(
    source: &Source,
    operator: &Token,
    left: Option<Gap>,
    right: Option<Gap>,
) -> Option<(String, Vec<Edit>)> {
    let left_needed = left.is_some_and(|gap| {
        gap.before.kind == TokenKind::Number || source.text_of(gap.before).ends_with('.')
    });
    let right_needed = right.is_some_and(|gap| source.text_of(gap.after).starts_with('.'));
    let edits: Vec<Edit> = [(left, left_needed), (right, right_needed)]
        .into_iter()
        .filter_map(|(gap, needed)| gap.filter(|gap| !needed && !gap.is_empty()))
        .map(|gap| Edit::remove(gap.range()))
        .collect();
    if edits.is_empty() {
        return None;
    }
    let text = source.text_of(operator);
    Some((format!("Space around operator `{text}` detected."), edits))
}

inventory::submit! { Registration(|| Box::new(SpaceAroundOperators)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/SpaceAroundOperators";

    #[test]
    fn flags_missing_spaces() {
        expect_offense(
            READER,
            "local x=1\n",
            1,
            8,
            "Surrounding space missing for operator `=`.",
        );
        expect_offense(
            READER,
            "y = x+1\n",
            1,
            6,
            "Surrounding space missing for operator `+`.",
        );
        expect_offense(
            READER,
            "if a==b then end\n",
            1,
            5,
            "Surrounding space missing for operator `==`.",
        );
        expect_offense(
            READER,
            "if a !=b then end\n",
            1,
            6,
            "Surrounding space missing for operator `!=`.",
        );
        expect_offense(
            READER,
            "y = (a)and(b)\n",
            1,
            8,
            "Surrounding space missing for operator `and`.",
        );
        expect_offense(
            READER,
            "for i=1, 2 do end\n",
            1,
            6,
            "Surrounding space missing for operator `=`.",
        );
        expect_offense(
            READER,
            "t = { a=1 }\n",
            1,
            8,
            "Surrounding space missing for operator `=`.",
        );
    }

    #[test]
    fn flags_extra_spaces() {
        expect_offense(
            READER,
            "y = a  * b\n",
            1,
            8,
            "Operator `*` should be surrounded by a single space.",
        );
        expect_offense(
            READER,
            "y =  1\n",
            1,
            3,
            "Operator `=` should be surrounded by a single space.",
        );
        expect_offense(
            READER,
            "y\t= 1\n",
            1,
            3,
            "Operator `=` should be surrounded by a single space.",
        );
    }

    #[test]
    fn accepts_well_spaced_operators() {
        expect_no_offenses(
            READER,
            "local x = a + b * c - d / e % f ^ g\nif a == b and c != d or e ~= f then end\n",
        );
        expect_no_offenses(
            READER,
            "if a < b and b <= c and c > d and d >= e then end\n",
        );
        expect_no_offenses(READER, "local s = 'a'..b..'c'\n");
        expect_no_offenses(READER, "x = y\n  + z\nx = y +\n  z\n");
        expect_no_offenses(READER, "x = y -- a=b\ns = 'a=b+c'\n");
    }

    #[test]
    fn leaves_unary_operators_alone() {
        expect_no_offenses(READER, "x = -1\ny = #t\nz = !a\nw = not b\nf(-x, a - -b)\n");
        expect_no_offenses(READER, "return -x\nx = t[-1]\nx = {-1}\nx = 2 ^ -1\n");
        expect_offense(
            READER,
            "x = f()-1\n",
            1,
            8,
            "Surrounding space missing for operator `-`.",
        );
        expect_offense(
            READER,
            "x = t[1]-y\n",
            1,
            9,
            "Surrounding space missing for operator `-`.",
        );
    }

    #[test]
    fn flags_spaced_concatenation() {
        expect_offense(
            READER,
            "s = 'a' .. b\n",
            1,
            9,
            "Space around operator `..` detected.",
        );
        expect_offense(
            READER,
            "s = 'a'.. b\n",
            1,
            8,
            "Space around operator `..` detected.",
        );
        expect_no_offenses(READER, "s = 'a'..\n  b\ns = a\n  ..b\n");
    }

    #[test]
    fn keeps_spaces_that_separate_numbers_and_dots() {
        expect_no_offenses(READER, "s = 1 ..'x'\ns = a.. .5\ns = a.. ...\n");
        expect_correction(READER, "s = 1 .. x\n", "s = 1 ..x\n");
    }

    #[test]
    fn concat_style_space() {
        let options = "ConcatStyle: space";
        assert!(inspect_with(READER, "s = 'a' .. b\n", options).is_empty());
        let offenses = inspect_with(READER, "s = 'a'..b\n", options);
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Surrounding space missing for operator `..`."
        );
        assert_eq!(
            autocorrect_with(READER, "s = 'a'..b\n", options),
            "s = 'a' .. b\n"
        );
    }

    #[test]
    fn allows_aligned_equal_signs() {
        expect_no_offenses(READER, "local a   = 1\nlocal bcd = 2\n");
        expect_no_offenses(READER, "t = {\n  x     = 1,\n\n  long_ = 2\n}\n");
        expect_no_offenses(
            READER,
            "TK_add    = byte '+'\nTK_assign = byte '='\nTK_band   = byte '&'\n",
        );
        expect_no_offenses(
            READER,
            "a.size  = a.size  or 80\na.icon  = a.icon  or 'x'\n-- note\na.color = a.color or c\n",
        );
        expect_no_offenses(READER, "cm = cm - m  * 100\nm  = m  - km * 1000\n");
        expect_offense(
            READER,
            "local a   = 1\nlocal bcde = 2\n",
            1,
            11,
            "Operator `=` should be surrounded by a single space.",
        );
        expect_offense(
            READER,
            "local a  = 1\nlocal bc =  2\n",
            2,
            10,
            "Operator `=` should be surrounded by a single space.",
        );
        assert_eq!(
            inspect_with(
                READER,
                "local a   = 1\nlocal bcd = 2\n",
                "AllowForAlignment: false"
            )
            .len(),
            1
        );
    }

    #[test]
    fn ignores_sides_next_to_comments() {
        expect_no_offenses(
            READER,
            "x = a + --[[c]] b\nx = --[[c]]-1\ny = 1 +  -- c\n  2\n",
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "x=a+b*c\n", "x = a + b * c\n");
        expect_correction(READER, "x  =  y\n", "x = y\n");
        expect_correction(READER, "s = 'a' ..  b  ..'c'\n", "s = 'a'..b..'c'\n");
        expect_correction(READER, "a   = 1\nbcd =2\n", "a   = 1\nbcd = 2\n");
        expect_correction(READER, "if x==-1 then end\n", "if x == -1 then end\n");
    }
}
