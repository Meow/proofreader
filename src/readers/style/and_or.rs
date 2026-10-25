//! `Style/AndOr`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Checks the spelling of the logical operators.
///
/// With `EnforcedStyle: keywords` (the default) GLua's C-style `&&` and `||` are flagged in
/// favour of `and` and `or`; the fix keeps one space on each side of the keyword, adding it where
/// the operator touched its operands. With `symbols`, `and` and `or` are flagged in favour of
/// `&&` and `||`, which bind exactly like them; the fix replaces the keyword only.
pub struct AndOr;

impl Reader for AndOr {
    fn name(&self) -> &'static str {
        "Style/AndOr"
    }

    fn description(&self) -> &'static str {
        "Checks that logical operators are spelled `and`/`or` (or `&&`/`||`)."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("keywords".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        if ctx.option_str("EnforcedStyle", "keywords") == "symbols" {
            symbols(ctx);
            return;
        }
        let source = ctx.source;
        let tokens = &source.tokens;
        for (index, token) in tokens.iter().enumerate() {
            let (keyword, operator) = match token.kind {
                TokenKind::AndAnd => ("and", "&&"),
                TokenKind::OrOr => ("or", "||"),
                _ => continue,
            };
            let spaced_before = index
                .checked_sub(1)
                .is_none_or(|before| tokens[before].is_trivia());
            let spaced_after = tokens
                .get(index + 1)
                .is_none_or(|after| after.is_trivia() || after.kind == TokenKind::Eof);
            let replacement = format!(
                "{}{keyword}{}",
                if spaced_before { "" } else { " " },
                if spaced_after { "" } else { " " }
            );
            ctx.add_offense_with_fix(
                token.range(),
                format!("Use `{keyword}` instead of `{operator}`."),
                vec![Edit::replace(token.range(), replacement)],
            );
        }
    }
}

/// Flags `and` and `or` in favour of `&&` and `||`.
fn symbols(ctx: &mut Context) {
    let source = ctx.source;
    for token in source.code_tokens() {
        let (operator, keyword) = match token.kind {
            TokenKind::And => ("&&", "and"),
            TokenKind::Or => ("||", "or"),
            _ => continue,
        };
        ctx.add_offense_with_fix(
            token.range(),
            format!("Use `{operator}` instead of `{keyword}`."),
            vec![Edit::replace(token.range(), operator)],
        );
    }
}

inventory::submit! { Registration(|| Box::new(AndOr)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/AndOr";

    #[test]
    fn flags_c_style_operators() {
        expect_offense(
            READER,
            "if a && b then end\n",
            1,
            6,
            "Use `and` instead of `&&`.",
        );
        expect_offense(READER, "x = a || b\n", 1, 7, "Use `or` instead of `||`.");
    }

    #[test]
    fn accepts_keywords_and_strings() {
        expect_no_offenses(READER, "if a and b or c then end\n");
        expect_no_offenses(READER, "s = 'a && b || c'\n-- a && b\n");
    }

    #[test]
    fn autocorrects_with_spacing() {
        expect_correction(READER, "if a && b then end\n", "if a and b then end\n");
        expect_correction(READER, "x = a||b\n", "x = a or b\n");
        expect_correction(READER, "x = (a)&&(b) || c\n", "x = (a) and (b) or c\n");
        expect_correction(READER, "x = a &&\n  b\n", "x = a and\n  b\n");
    }

    const SYMBOLS: &str = "EnforcedStyle: symbols";

    #[test]
    fn symbols_style_flags_keywords() {
        let offenses = inspect_with(READER, "if a and b or c then end\n", SYMBOLS);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 6, "Use `&&` instead of `and`."),
                (1, 12, "Use `||` instead of `or`.")
            ]
        );
        assert!(
            inspect_with(READER, "if a && b || c then end\ns = 'a and b'\n", SYMBOLS).is_empty()
        );
        assert!(inspect_with(READER, "-- a and b\nband = orange\n", SYMBOLS).is_empty());
    }

    #[test]
    fn symbols_style_autocorrects() {
        let corrected = autocorrect_with(READER, "x = (a)and(b) or\n  c\n", SYMBOLS);
        assert_eq!(corrected, "x = (a)&&(b) ||\n  c\n");
        assert_eq!(autocorrect_with(READER, &corrected, SYMBOLS), corrected);
    }

    #[test]
    fn symbols_style_ignores_markup() {
        assert!(
            inspect_lumen(READER, "return <text>and or</text>\n").is_empty(),
            "markup text is not code"
        );
    }
}
