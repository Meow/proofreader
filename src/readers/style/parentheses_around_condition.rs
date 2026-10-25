//! `Style/ParenthesesAroundCondition`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::naming::declarations::significant_tokens;
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Checks the parentheses around the conditions of `if`, `elseif`, `while` and `until`.
///
/// With `EnforcedStyle: omit` (the default) it flags `if (x) then`, `elseif (x) then`,
/// `while (x) do` and `until (x)` when one parenthesised group wraps the entire condition; the
/// fix removes the parentheses and the spaces just inside them, keeping a space between the
/// keyword and the condition.
///
/// With `require` (the vanilla Garry's Mod style, `if ( !IsValid( ply ) ) then`) every condition
/// must be one parenthesised expression; `if (a) and (b) then` is not, so it is flagged too. The
/// fix wraps the condition as written in parentheses (`if x then` becomes `if (x) then`);
/// `Layout/SpaceInsideParens` decides the spaces inside them. Files that do not parse are skipped
/// in this style.
pub struct ParenthesesAroundCondition;

impl Reader for ParenthesesAroundCondition {
    fn name(&self) -> &'static str {
        "Style/ParenthesesAroundCondition"
    }

    fn description(&self) -> &'static str {
        "Checks for (or requires) parentheses around the condition of `if`, `elseif`, `while` and `until`."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("EnforcedStyle", Yaml::String("omit".to_owned()))]
    }

    fn investigate(&self, ctx: &mut Context) {
        if ctx.option_str("EnforcedStyle", "omit") == "require" {
            require(ctx);
            return;
        }
        let source = ctx.source;
        let tokens = significant_tokens(source);
        for (index, keyword) in tokens.iter().enumerate() {
            let article = match keyword.kind {
                TokenKind::If | TokenKind::ElseIf | TokenKind::Until => "an",
                TokenKind::While => "a",
                _ => continue,
            };
            let Some(open) = tokens
                .get(index + 1)
                .filter(|t| t.kind == TokenKind::LParen)
            else {
                continue;
            };
            let Some(close_index) = matching_paren(&tokens, index + 1) else {
                continue;
            };
            if close_index == index + 2
                || !ends_condition(keyword.kind, tokens.get(close_index + 1))
            {
                continue;
            }
            let close = tokens[close_index];
            ctx.add_offense_with_fix(
                open.start..close.end,
                format!(
                    "Don't use parentheses around the condition of {article} `{}`.",
                    source.text_of(keyword)
                ),
                vec![remove_open(source, open), remove_close(source, &close)],
            );
        }
    }
}

/// Flags conditions that are not one parenthesised expression.
fn require(ctx: &mut Context) {
    let Ok(block) = parse(ctx.source) else {
        return;
    };
    let mut finder = Unwrapped {
        source: ctx.source,
        found: Vec::new(),
    };
    walk(&block, &mut finder);
    for (condition, keyword) in finder.found {
        let article = if keyword == "while" { "a" } else { "an" };
        ctx.add_offense_with_fix(
            condition.clone(),
            format!("Use parentheses around the condition of {article} `{keyword}`."),
            vec![
                Edit::insert(condition.start, "("),
                Edit::insert(condition.end, ")"),
            ],
        );
    }
}

/// Collects the conditions that are not wrapped in parentheses, with their keyword.
struct Unwrapped<'a> {
    source: &'a Source,
    found: Vec<(std::ops::Range<usize>, &'a str)>,
}

impl Unwrapped<'_> {
    /// Records `condition` of `keyword` unless it is parenthesised.
    fn check(&mut self, condition: &Expression, keyword: &'static str) {
        if !matches!(condition.kind, ExpressionKind::Parenthesized(_)) {
            self.found.push((condition.range.clone(), keyword));
        }
    }
}

impl<'t> Visitor<'t> for Unwrapped<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::If { clauses, .. } => {
                for clause in clauses {
                    let keyword = if self.source.text_of(&clause.keyword) == "if" {
                        "if"
                    } else {
                        "elseif"
                    };
                    self.check(&clause.condition, keyword);
                }
            }
            StatementKind::While { condition, .. } => self.check(condition, "while"),
            StatementKind::Repeat { condition, .. } => self.check(condition, "until"),
            _ => {}
        }
    }
}

/// Index of the `)` matching the `(` at `open`.
fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `next`, the token after the closing parenthesis, ends the condition of `keyword`.
fn ends_condition(keyword: TokenKind, next: Option<&Token>) -> bool {
    match keyword {
        TokenKind::If | TokenKind::ElseIf => next.is_some_and(|t| t.kind == TokenKind::Then),
        TokenKind::While => next.is_some_and(|t| t.kind == TokenKind::Do),
        _ => next.is_none_or(|t| !continues_expression(t.kind)),
    }
}

/// Whether a token of `kind` after a parenthesised expression continues that expression.
fn continues_expression(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::AndAnd
            | TokenKind::OrOr
            | TokenKind::Concat
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::String { .. }
    )
}

/// Removes `(` and the spaces after it on its line, leaving one space if the keyword touches it.
fn remove_open(source: &Source, open: &Token) -> Edit {
    let bytes = source.text.as_bytes();
    let end = open.end
        + bytes[open.end..]
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
    let glued = open
        .start
        .checked_sub(1)
        .is_some_and(|before| !bytes[before].is_ascii_whitespace());
    Edit::replace(open.start..end, if glued { " " } else { "" })
}

/// Removes `)` and the spaces before it on its line, leaving one space if the next token
/// touches it.
fn remove_close(source: &Source, close: &Token) -> Edit {
    let bytes = source.text.as_bytes();
    let start = close.start
        - bytes[..close.start]
            .iter()
            .rev()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
    let glued = bytes
        .get(close.end)
        .is_some_and(|after| !after.is_ascii_whitespace());
    Edit::replace(start..close.end, if glued { " " } else { "" })
}

inventory::submit! { Registration(|| Box::new(ParenthesesAroundCondition)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/ParenthesesAroundCondition";
    const IF: &str = "Don't use parentheses around the condition of an `if`.";

    #[test]
    fn flags_wrapped_conditions() {
        expect_offense(READER, "if (x) then end\n", 1, 4, IF);
        expect_offense(
            READER,
            "if a then\nelseif (b == 1) then\nend\n",
            2,
            8,
            "Don't use parentheses around the condition of an `elseif`.",
        );
        expect_offense(
            READER,
            "while (f(x)) do end\n",
            1,
            7,
            "Don't use parentheses around the condition of a `while`.",
        );
        expect_offense(
            READER,
            "repeat\n  x()\nuntil (done)\nnext()\n",
            3,
            7,
            "Don't use parentheses around the condition of an `until`.",
        );
        expect_offense(READER, "if (!a and (b or c)) then end\n", 1, 4, IF);
    }

    #[test]
    fn accepts_partial_parentheses() {
        expect_no_offenses(
            READER,
            "if (a + b) < c then end\nif (a) and (b) then end\nif (f)(x) then end\n\
             while (a).b do end\nif x then end\nrepeat until (a) == b\nrepeat until (a)..b\n",
        );
        expect_no_offenses(READER, "f(x)\nlocal t = (a)\nif x then y = (z) end\n");
        expect_no_offenses(READER, "if () then end\nif (x\n");
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "if (x) then end\n", "if x then end\n");
        expect_correction(READER, "if(x)then end\n", "if x then end\n");
        expect_correction(READER, "if ( x ) then end\n", "if x then end\n");
        expect_correction(READER, "if ((x)) then end\n", "if x then end\n");
        expect_correction(READER, "while (a and b) do end\n", "while a and b do end\n");
        expect_correction(READER, "repeat until (a)\n", "repeat until a\n");
    }

    const REQUIRE: &str = "EnforcedStyle: require";

    fn corrects(src: &str, expected: &str) {
        let corrected = autocorrect_with(READER, src, REQUIRE);
        assert_eq!(corrected, expected, "correction of {src:?}");
        assert_eq!(autocorrect_with(READER, &corrected, REQUIRE), corrected);
    }

    #[test]
    fn require_style_flags_bare_conditions() {
        let src = "if x then\nelseif a == b then\nend\nwhile f(x) do end\nrepeat until done\n";
        let offenses = inspect_with(READER, src, REQUIRE);
        let found: Vec<(u32, u32, &str)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col, offense.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, 4, "Use parentheses around the condition of an `if`."),
                (2, 8, "Use parentheses around the condition of an `elseif`."),
                (4, 7, "Use parentheses around the condition of a `while`."),
                (5, 14, "Use parentheses around the condition of an `until`."),
            ]
        );
    }

    #[test]
    fn require_style_accepts_wrapped_conditions() {
        assert!(
            inspect_with(
                READER,
                "if ( !IsValid( ply ) ) then return end\nif ((a) and (b)) then end\n\
                 while (x) do end\nrepeat until (y)\nif (\n  a\n) then end\n",
                REQUIRE
            )
            .is_empty()
        );
        assert!(
            inspect_with(READER, "if x then\n", REQUIRE).is_empty(),
            "unparsable"
        );
    }

    #[test]
    fn require_style_flags_partial_parentheses() {
        let offenses = inspect_with(READER, "if (a) and (b) then end\n", REQUIRE);
        assert_eq!(offenses.len(), 1);
        assert_eq!((offenses[0].col, offenses[0].last_col), (4, 14));
        corrects("if (a) and (b) then end\n", "if ((a) and (b)) then end\n");
        corrects("if (f)(x) then end\n", "if ((f)(x)) then end\n");
    }

    #[test]
    fn require_style_autocorrects() {
        corrects("if !x then return end\n", "if (!x) then return end\n");
        corrects("while a and\n  b do end\n", "while (a and\n  b) do end\n");
        corrects("repeat x() until done\n", "repeat x() until (done)\n");
        corrects(
            "if a then\nelseif b then\nend\n",
            "if (a) then\nelseif (b) then\nend\n",
        );
        corrects(
            "local f = function() if x then end end\n",
            "local f = function() if (x) then end end\n",
        );
    }

    #[test]
    fn lumen_templates_are_checked() {
        let corrected = autocorrect_lumen(
            READER,
            "return <view>{f(function() if x then end end)}</view>\n",
        );
        assert_eq!(
            corrected, "return <view>{f(function() if x then end end)}</view>\n",
            "default style"
        );
        let offenses = inspect_lumen(READER, "if (x) then end\nreturn <view/>\n");
        assert_eq!(offenses.len(), 1);
    }
}
