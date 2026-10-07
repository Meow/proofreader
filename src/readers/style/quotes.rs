//! String quoting helpers, shared by the style readers.

/// Converts a complete double-quoted string literal to single quotes, unescaping `\"`.
///
/// Returns `None` when `literal` is not a terminated double-quoted string or when its content
/// contains a single quote (escaped or not), in which case double quotes are preferred.
pub fn to_single_quoted(literal: &str) -> Option<String> {
    requote(literal, '"', '\'')
}

/// Converts a complete single-quoted string literal to double quotes, unescaping `\'`.
///
/// Returns `None` when `literal` is not a terminated single-quoted string or when its content
/// contains a double quote (escaped or not), in which case single quotes are preferred.
pub fn to_double_quoted(literal: &str) -> Option<String> {
    requote(literal, '\'', '"')
}

/// Re-quotes a terminated `from`-quoted literal with `to` quotes, unescaping `\from`; `None`
/// when the content contains `to`.
fn requote(literal: &str, from: char, to: char) -> Option<String> {
    let inner = literal.strip_prefix(from)?.strip_suffix(from)?;
    let escaping_backslashes = inner.chars().rev().take_while(|&c| c == '\\').count();
    if escaping_backslashes % 2 == 1 || inner.contains(to) {
        return None;
    }
    let mut out = String::with_capacity(literal.len());
    out.push(to);
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(next) if next == from => out.push(from),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out.push(to);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_quotes() {
        assert_eq!(to_single_quoted(r#""abc""#).as_deref(), Some("'abc'"));
        assert_eq!(to_single_quoted(r#""""#).as_deref(), Some("''"));
        assert_eq!(
            to_single_quoted(r#""say \"hi\"""#).as_deref(),
            Some(r#"'say "hi"'"#)
        );
        assert_eq!(
            to_single_quoted(r#""a\\b\n""#).as_deref(),
            Some(r"'a\\b\n'")
        );
    }

    #[test]
    fn swaps_to_double_quotes() {
        assert_eq!(to_double_quoted("'abc'").as_deref(), Some("\"abc\""));
        assert_eq!(to_double_quoted(r"'it\'s'").as_deref(), Some(r#""it's""#));
        assert_eq!(to_double_quoted(r#"'say "hi"'"#), None);
        assert_eq!(to_double_quoted(r"'esc\'"), None);
        assert_eq!(to_double_quoted("\"abc\""), None);
    }

    #[test]
    fn keeps_strings_that_need_double_quotes() {
        assert_eq!(to_single_quoted(r#""it's""#), None);
        assert_eq!(to_single_quoted(r#""it\'s""#), None);
        assert_eq!(to_single_quoted(r#""open"#), None);
        assert_eq!(to_single_quoted(r#""esc\""#), None);
        assert_eq!(to_single_quoted("'single'"), None);
        assert_eq!(to_single_quoted("\""), None);
    }
}
