//! `Style/TrailingCommaInTable`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::declarations::significant_tokens;
use crate::token::TokenKind;

/// Checks the comma after the last item of a table constructor.
///
/// With `EnforcedStyle: no_comma` (the default) a `,` directly before `}` is flagged. With
/// `comma`, multi-line tables (the `}` on a later line than the last item) need one and
/// single-line tables must not have one. A trailing `;` is left to `Style/Semicolon`.
pub struct TrailingCommaInTable;

impl Reader for TrailingCommaInTable {
    fn name(&self) -> &'static str {
        "Style/TrailingCommaInTable"
    }

    fn description(&self) -> &'static str {
        "Checks for a trailing comma after the last item of a table."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("no_comma".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let comma_style = ctx.option_str("EnforcedStyle", "no_comma") == "comma";
        let tokens = significant_tokens(ctx.source);
        for pair in tokens.windows(2) {
            let (last, close) = (pair[0], pair[1]);
            if close.kind != TokenKind::RBrace {
                continue;
            }
            let multiline = last.end_line < close.line;
            match last.kind {
                TokenKind::Comma if !comma_style => {
                    ctx.add_offense_with_fix(
                        last.range(),
                        "Avoid comma after the last item of a table.",
                        vec![Edit::remove(last.range())],
                    );
                }
                TokenKind::Comma if !multiline => {
                    ctx.add_offense_with_fix(
                        last.range(),
                        "Avoid comma after the last item of a single-line table.",
                        vec![Edit::remove(last.range())],
                    );
                }
                TokenKind::Comma | TokenKind::Semicolon | TokenKind::LBrace => {}
                _ if comma_style && multiline => {
                    ctx.add_offense_with_fix(
                        last.range(),
                        "Put a comma after the last item of a multiline table.",
                        vec![Edit::insert(last.end, ",")],
                    );
                }
                _ => {}
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(TrailingCommaInTable)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/TrailingCommaInTable";
    const AVOID: &str = "Avoid comma after the last item of a table.";
    const COMMA: &str = "EnforcedStyle: comma";

    #[test]
    fn flags_trailing_commas() {
        expect_offense(READER, "t = { 1, 2, }\n", 1, 11, AVOID);
        expect_offense(READER, "t = {\n  a = 1,\n  b = 2,\n}\n", 3, 8, AVOID);
        expect_offense(READER, "t = {\n  a = 1, -- note\n}\n", 2, 8, AVOID);
    }

    #[test]
    fn accepts_tables_without_trailing_commas() {
        expect_no_offenses(
            READER,
            "t = { 1, 2 }\nu = {}\nv = {\n  a = 1\n}\nw = { 1; }\n",
        );
        expect_no_offenses(READER, "f(a, b)\ns = ',}'\n-- { a, }\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "t = { 1, 2, }\n", "t = { 1, 2 }\n");
        expect_correction(READER, "t = {\n  a = 1,\n}\n", "t = {\n  a = 1\n}\n");
        expect_correction(READER, "t = { { 1, }, }\n", "t = { { 1 } }\n");
    }

    #[test]
    fn comma_style() {
        let src = "t = {\n  a = 1\n}\nu = { 1, }\nv = {\n  b = 2, -- c\n}\nw = { 1 }\n";
        let offenses = inspect_with(READER, src, COMMA);
        let found: Vec<(u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (2, "Put a comma after the last item of a multiline table."),
                (4, "Avoid comma after the last item of a single-line table."),
            ]
        );
        assert_eq!(
            autocorrect_with(READER, "t = {\n  a = 1 -- c\n}\n", COMMA),
            "t = {\n  a = 1, -- c\n}\n"
        );
    }
}
