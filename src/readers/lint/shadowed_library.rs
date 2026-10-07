//! `Lint/ShadowedLibrary`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::libraries::GMOD_LIBRARIES;
use crate::readers::naming::declarations::{
    Declaration, DeclarationKind, declarations, significant_tokens,
};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// Flags local variables, `local function` names, parameters and loop variables named after a
/// GMod library, which hides the library for the rest of the scope. A plain alias of the library
/// itself (`local render = render`) hides nothing and is allowed.
pub struct ShadowedLibrary;

impl Reader for ShadowedLibrary {
    fn name(&self) -> &'static str {
        "Lint/ShadowedLibrary"
    }

    fn description(&self) -> &'static str {
        "Checks for variables and parameters named after a GMod library."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        let libraries = GMOD_LIBRARIES
            .iter()
            .map(|name| Yaml::String((*name).to_owned()))
            .collect();
        vec![("Libraries", Yaml::Array(libraries))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let libraries = ctx.option_str_list("Libraries", GMOD_LIBRARIES);
        let source = ctx.source;
        let tokens = significant_tokens(source);
        for declaration in declarations(source) {
            let name = source.text_of(&declaration.token);
            if libraries.iter().any(|library| library == name)
                && !is_alias(source, &tokens, &declaration)
            {
                ctx.add_offense(
                    declaration.token.range(),
                    format!("Variable `{name}` shadows the GMod `{name}` library."),
                );
            }
        }
    }
}

/// Whether `declaration` is `local name = name` with nothing else on either side.
fn is_alias(source: &Source, tokens: &[Token], declaration: &Declaration) -> bool {
    if declaration.kind != DeclarationKind::Local {
        return false;
    }
    let index = tokens.partition_point(|token| token.start < declaration.token.start);
    let kinds = |offset: usize| tokens.get(index + offset).map(|token| token.kind);
    let name = source.text_of(&declaration.token);
    kinds(1) == Some(TokenKind::Assign)
        && tokens
            .get(index + 2)
            .is_some_and(|value| value.kind == TokenKind::Name && source.text_of(value) == name)
        && !matches!(
            kinds(3),
            Some(
                TokenKind::Dot
                    | TokenKind::Colon
                    | TokenKind::LBracket
                    | TokenKind::LParen
                    | TokenKind::LBrace
                    | TokenKind::String { .. }
                    | TokenKind::Comma
                    | TokenKind::Or
                    | TokenKind::And
                    | TokenKind::OrOr
                    | TokenKind::AndAnd
                    | TokenKind::Concat
            )
        )
        && index
            .checked_sub(1)
            .and_then(|before| tokens.get(before))
            .is_some_and(|before| before.kind == TokenKind::Local)
}

inventory::submit! { Registration(|| Box::new(ShadowedLibrary)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/ShadowedLibrary";

    fn message(name: &str) -> String {
        format!("Variable `{name}` shadows the GMod `{name}` library.")
    }

    #[test]
    fn flags_locals() {
        expect_offense(READER, "local player = x\n", 1, 7, &message("player"));
        expect_offense(READER, "local a, file = 1, 2\n", 1, 10, &message("file"));
        expect_offense(
            READER,
            "local function table() end\n",
            1,
            16,
            &message("table"),
        );
    }

    #[test]
    fn flags_parameters_and_loop_variables() {
        expect_offense(
            READER,
            "function f(a, player) end\n",
            1,
            15,
            &message("player"),
        );
        expect_offense(
            READER,
            "hook.Add('X', 'y', function(sound) end)\n",
            1,
            29,
            &message("sound"),
        );
        expect_offense(
            READER,
            "for _, player in ipairs(t) do end\n",
            1,
            8,
            &message("player"),
        );
        expect_offense(READER, "for team = 1, 2 do end\n", 1, 5, &message("team"));
    }

    #[test]
    fn ignores_fields_and_uses() {
        expect_no_offenses(
            READER,
            "self.player = 1\nlocal x = t.file\nfunction obj:player() end\nplayer.GetAll()\n\
             local a = { player = 1 }\nfunction x.file() end\n",
        );
        expect_no_offenses(
            READER,
            "local list, system, render_target = 1\n-- local player\n",
        );
    }

    #[test]
    fn allows_aliasing_the_library() {
        expect_no_offenses(
            READER,
            "local render = render
local draw = draw -- cache
",
        );
        expect_offense(
            READER,
            "local render = render.x
",
            1,
            7,
            &message("render"),
        );
        expect_offense(
            READER,
            "local render = render or 1
",
            1,
            7,
            &message("render"),
        );
        expect_offense(
            READER,
            "local render = draw
",
            1,
            7,
            &message("render"),
        );
        expect_offense(
            READER,
            "local a, render = 1, render
",
            1,
            10,
            &message("render"),
        );
        expect_offense(
            READER,
            "local render = render, 1
",
            1,
            7,
            &message("render"),
        );
    }

    #[test]
    fn libraries_option_replaces_the_default_list() {
        let src = "local list = 1\nlocal player = 2\n";
        let offenses = inspect_with(READER, src, "Libraries: [list]");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, message("list"));
        assert_eq!(offenses[0].line, 1);
    }

    #[test]
    fn never_autocorrects() {
        let src = "local player = 1\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
