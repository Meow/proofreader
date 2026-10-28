//! `Style/Not`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::{Token, TokenKind};

/// Checks the spelling of logical negation.
///
/// With `EnforcedStyle: bang` (the default) the `not` keyword is flagged in favour of GLua's `!`:
/// the fix replaces `not` and the spaces after it with `!`, so `not x` becomes `!x` and
/// `not (x)` becomes `!(x)`. With `keyword`, `!` is flagged in favour of `not`: the fix replaces
/// `!` and the spaces after it with `not ` (`!x`, `! x` and `!!x` become `not x` and
/// `not not x`), adding a space before `not` when the token before touches it (`return!x`). A line
/// break after the operator is kept in both styles.
pub struct Not;

impl Reader for Not {
    fn name(&self) -> &'static str {
        "Style/Not"
    }

    fn description(&self) -> &'static str {
        "Checks that logical negation is spelled `!` (or `not`)."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("bang".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        if ctx.option_str("EnforcedStyle", "bang") == "keyword" {
            keyword(ctx);
            return;
        }
        let tokens = &ctx.source.tokens;
        for (index, token) in tokens.iter().enumerate() {
            if token.kind != TokenKind::Not {
                continue;
            }
            let end = spaces_end(tokens, index);
            ctx.add_offense_with_fix(
                token.range(),
                "Use `!` instead of `not`.",
                vec![Edit::replace(token.start..end, "!")],
            );
        }
    }
}

/// The end of the spaces and tabs directly after the token at `index`, or the end of that token.
fn spaces_end(tokens: &[Token], index: usize) -> usize {
    tokens[index + 1..]
        .iter()
        .take_while(|next| matches!(next.kind, TokenKind::Space | TokenKind::Tab))
        .last()
        .map_or(tokens[index].end, |space| space.end)
}

/// Flags `!` in favour of `not`.
fn keyword(ctx: &mut Context) {
    let source = ctx.source;
    let tokens = &source.tokens;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != TokenKind::Bang {
            continue;
        }
        let end = spaces_end(tokens, index);
        let line_break = tokens
            .get(index + 1)
            .is_some_and(|next| next.kind == TokenKind::Newline)
            || (end > token.end
                && tokens
                    .iter()
                    .skip(index + 1)
                    .find(|next| next.start >= end)
                    .is_some_and(|next| next.kind == TokenKind::Newline));
        let glued = index.checked_sub(1).is_some_and(|before| {
            source
                .text_of(&tokens[before])
                .ends_with(|c: char| c.is_alphanumeric() || c == '_')
        });
        let replacement = format!(
            "{}not{}",
            if glued { " " } else { "" },
            if line_break { "" } else { " " }
        );
        let range = if line_break {
            token.range()
        } else {
            token.start..end
        };
        ctx.add_offense_with_fix(
            token.range(),
            "Use `not` instead of `!`.",
            vec![Edit::replace(range, replacement)],
        );
    }
}

inventory::submit! { Registration(|| Box::new(Not)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/Not";
    const MESSAGE: &str = "Use `!` instead of `not`.";

    #[test]
    fn flags_not() {
        expect_offense(READER, "if not x then end\n", 1, 4, MESSAGE);
        expect_offenses(
            READER,
            "y = not not x\n",
            &[(1, 5, MESSAGE), (1, 9, MESSAGE)],
        );
    }

    #[test]
    fn looks_inside_the_extended_syntax() {
        expect_correction(
            READER,
            "f(|x| -> not x, c ? not a : b)\n",
            "f(|x| -> !x, c ? !a : b)\n",
        );
    }

    #[test]
    fn accepts_bang_and_strings() {
        expect_no_offenses(READER, "if !x then end\n");
        expect_no_offenses(READER, "s = 'not x'\n-- not x\nnothing = notable\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "if not x then end\n", "if !x then end\n");
        expect_correction(READER, "y = not (a or b)\n", "y = !(a or b)\n");
        expect_correction(READER, "y = not(a)\n", "y = !(a)\n");
        expect_correction(READER, "y = not \t x\n", "y = !x\n");
        expect_correction(READER, "y = not not x\n", "y = !!x\n");
        expect_correction(READER, "y = not\n  x\n", "y = !\n  x\n");
    }

    const KEYWORD: &str = "EnforcedStyle: keyword";

    fn corrects(src: &str, expected: &str) {
        let corrected = autocorrect_with(READER, src, KEYWORD);
        assert_eq!(corrected, expected, "correction of {src:?}");
        assert_eq!(autocorrect_with(READER, &corrected, KEYWORD), corrected);
    }

    #[test]
    fn keyword_style_flags_bang() {
        let offenses = inspect_with(READER, "if !x and not y then end\n", KEYWORD);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].line, offenses[0].col), (1, 4));
        assert_eq!(offenses[0].message, "Use `not` instead of `!`.");
        assert!(inspect_with(READER, "if a != b then end\ns = '!x'\n-- !x\n", KEYWORD).is_empty());
    }

    #[test]
    fn keyword_style_autocorrects() {
        corrects("if !x then end\n", "if not x then end\n");
        corrects("y = ! x\n", "y = not x\n");
        corrects("y = !!x\n", "y = not not x\n");
        corrects("y = !(a or b)\n", "y = not (a or b)\n");
        corrects("return!x\n", "return not x\n");
        corrects("y = (!x)\n", "y = (not x)\n");
        corrects("y = !\n  x\n", "y = not\n  x\n");
        corrects("y = ! \n  x\n", "y = not \n  x\n");
    }

    #[test]
    fn keyword_style_converges_with_space_after_not() {
        use crate::config::Config;
        use crate::runner::{Options, inspect_source};
        use crate::source::Source;
        let config = Config::from_yaml_str(
            "Style/Not:\n  EnforcedStyle: keyword\n",
            std::path::Path::new("."),
        )
        .expect("valid");
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let (_, corrected) = inspect_source(
            &Source::new("t.lua", "local y = ! x and !!z\n\nreturn y\n"),
            &config,
            &options,
        );
        assert_eq!(
            corrected.as_deref(),
            Some("local y = not x and not not z\n\nreturn y\n")
        );
    }
}
