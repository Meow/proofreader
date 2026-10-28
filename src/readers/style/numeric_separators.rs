//! `Style/NumericSeparators`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags decimal integer literals with at least `MinDigits` digits written without `_`
/// separators (`1000000`), and literals whose separators do not split the digits into groups
/// of three from the right (`1_0000`). The fix writes a `_` every three digits from the right
/// (`1_000_000`, `10_000`); LuaJIT 3.0 ignores the separators, so the value is the same. When
/// the rest of the line is aligned with a run of spaces, the run gives up as many spaces as the
/// separators add, so what follows stays in its column.
/// Hexadecimal literals, literals with a fraction or exponent and literals with a suffix
/// (`10LL`) are left alone. Needs a LuaJIT with the syntax extensions of LuaJIT 3.0; disabled by
/// default.
pub struct NumericSeparators;

impl Reader for NumericSeparators {
    fn name(&self) -> &'static str {
        "Style/NumericSeparators"
    }

    fn description(&self) -> &'static str {
        "Checks that long decimal integers are written with `_` between groups of three digits."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("MinDigits", Yaml::Integer(5))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let min_digits = ctx.option_usize("MinDigits", 5);
        let source = ctx.source;
        for (index, token) in source.tokens.iter().enumerate() {
            if token.kind != TokenKind::Number {
                continue;
            }
            let text = source.text_of(token);
            let Some(grouped) = grouped(text, min_digits) else {
                continue;
            };
            let mut edits = vec![Edit::replace(token.range(), grouped.clone())];
            if let Some(added) = grouped.len().checked_sub(text.len())
                && added > 0
                && let Some(padding) = source.tokens[index + 1..]
                    .iter()
                    .take_while(|next| next.line == token.line && next.kind != TokenKind::Newline)
                    .find(|next| next.kind == TokenKind::Space && next.len() > 1)
                    .filter(|padding| padding.len() > added)
            {
                edits.push(Edit::remove(padding.start..padding.start + added));
            }
            ctx.add_offense_with_fix(
                token.range(),
                format!("Separate the digits of `{text}` in groups of three: `{grouped}`."),
                edits,
            );
        }
    }
}

/// The literal `text` with its digits in groups of three, when it is a decimal integer that
/// needs them (at least `min_digits` digits, or separators already) and is not written so.
fn grouped(text: &str, min_digits: usize) -> Option<String> {
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'_')
    {
        return None;
    }
    let digits: Vec<char> = text.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < min_digits && !text.contains('_') {
        return None;
    }
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.iter().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push('_');
        }
        result.push(*digit);
    }
    (result != text).then_some(result)
}

inventory::submit! { Registration(|| Box::new(NumericSeparators)) }

#[cfg(test)]
mod tests {
    use super::grouped;
    use crate::testing::*;

    const READER: &str = "Style/NumericSeparators";
    const ENABLED: &str = "Enabled: true";

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn groups_digits_in_threes() {
        assert_eq!(grouped("10000", 5).as_deref(), Some("10_000"));
        assert_eq!(grouped("1000000", 5).as_deref(), Some("1_000_000"));
        assert_eq!(grouped("123456", 5).as_deref(), Some("123_456"));
        assert_eq!(grouped("1_0000", 5).as_deref(), Some("10_000"));
        assert_eq!(grouped("10_00", 5).as_deref(), Some("1_000"));
        assert_eq!(grouped("1000", 5), None);
        assert_eq!(grouped("1_000", 5), None);
        assert_eq!(grouped("10_000", 5), None);
        assert_eq!(grouped("1000", 4).as_deref(), Some("1_000"));
        for text in [
            "0xFFFFFF", "1.5e10", "100000.5", "100000LL", "1e100000", "0x_FF_FF",
        ] {
            assert_eq!(grouped(text, 5), None, "{text}");
        }
    }

    #[test]
    fn flags_and_corrects_literals() {
        let offenses = inspect_with(READER, "x = 100000\n", ENABLED);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 5));
        assert_eq!(
            offenses[0].message,
            "Separate the digits of `100000` in groups of three: `100_000`."
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "x = { 1000000, -25000, 1_0000, 1234, 0x10000, '100000' }\n",
                ENABLED
            ),
            "x = { 1_000_000, -25_000, 10_000, 1234, 0x10000, '100000' }\n"
        );
        assert_eq!(
            autocorrect_with(READER, "x = 1234\n", "Enabled: true\nMinDigits: 4"),
            "x = 1_234\n"
        );
        assert_eq!(
            autocorrect_with(
                READER,
                "t = {\n  [1000]   = 'a',\n  [10000]  = 'b',\n  [100000] = 'c'\n}\nx = 10000  -- note\n",
                ENABLED
            ),
            "t = {\n  [1000]   = 'a',\n  [10_000] = 'b',\n  [100_000] = 'c'\n}\nx = 10_000 -- note\n"
        );
    }
}
