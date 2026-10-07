//! `Style/DocumentationMethod`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::naming::declarations::{FunctionDefinition, function_definitions};
use crate::source::Source;
use crate::token::TokenKind;

/// Requires a doc comment directly before every named function definition.
///
/// The lines right above the definition must be `--` comment lines, at least one of which starts
/// with `---` (a doc string); blank lines may not separate them from the definition. Anonymous
/// functions are exempt, and so are `local function`s unless `RequireForLocalFunctions` is set.
pub struct DocumentationMethod;

impl Reader for DocumentationMethod {
    fn name(&self) -> &'static str {
        "Style/DocumentationMethod"
    }

    fn description(&self) -> &'static str {
        "Checks that function definitions are preceded by a doc comment."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("RequireForLocalFunctions", Yaml::Boolean(false))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let require_local = ctx.option_bool("RequireForLocalFunctions", false);
        let source = ctx.source;
        for definition in function_definitions(source) {
            if (definition.local && !require_local) || documented(source, &definition) {
                continue;
            }
            ctx.add_offense(
                definition.start.start..definition.last.end,
                format!("Missing documentation comment for `{}`.", definition.name),
            );
        }
    }
}

/// Whether the comment-only `--` lines directly above `definition` include a `---` doc line.
fn documented(source: &Source, definition: &FunctionDefinition) -> bool {
    let line = definition.start.line;
    if source
        .first_code_token_on_line(line)
        .is_none_or(|first| first.start != definition.start.start)
    {
        return false;
    }
    (1..line)
        .rev()
        .map_while(|above| comment_line(source, above))
        .any(|comment| comment.starts_with("---") && !comment[3..].starts_with('-'))
}

/// The text of line `n` when the line holds nothing but a `--` line comment.
fn comment_line(source: &Source, n: u32) -> Option<&str> {
    let first = source.first_code_token_on_line(n)?;
    let last = source.last_code_token_on_line(n)?;
    let text = source.text_of(first);
    (first == last && first.kind == (TokenKind::Comment { long: false }) && text.starts_with("--"))
        .then_some(text)
}

inventory::submit! { Registration(|| Box::new(DocumentationMethod)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/DocumentationMethod";

    #[test]
    fn flags_undocumented_functions() {
        expect_offense(
            READER,
            "function foo()\nend\n",
            1,
            1,
            "Missing documentation comment for `foo`.",
        );
        expect_offense(
            READER,
            "x = 1\nfunction a.b:c(d)\nend\n",
            2,
            1,
            "Missing documentation comment for `a.b:c`.",
        );
    }

    #[test]
    fn accepts_documented_functions() {
        expect_no_offenses(
            READER,
            "--- Does things.\n-- @param a [Number]\n-- @return [Number]\nfunction foo(a)\nend\n",
        );
        expect_no_offenses(READER, "--- Does things.\nfunction Foo:bar()\nend\n");
        expect_no_offenses(
            READER,
            "if x then\n  --- Nested.\n  function foo() end\nend\n",
        );
        expect_no_offenses(READER, "-- @internal\n--- Summary.\nfunction foo() end\n");
    }

    #[test]
    fn rejects_separated_or_non_doc_comments() {
        let message = "Missing documentation comment for `foo`.";
        expect_offense(READER, "--- Doc.\n\nfunction foo() end\n", 3, 1, message);
        expect_offense(
            READER,
            "-- Not a doc string.\nfunction foo() end\n",
            2,
            1,
            message,
        );
        expect_offense(
            READER,
            "--------------\nfunction foo() end\n",
            2,
            1,
            message,
        );
        expect_offense(READER, "--[[ Doc. ]]\nfunction foo() end\n", 2, 1, message);
        expect_offense(
            READER,
            "--- Doc.\nx = 1 -- c\nfunction foo() end\n",
            3,
            1,
            message,
        );
        expect_offense(
            READER,
            "--- Doc.\n// c\nfunction foo() end\n",
            3,
            1,
            message,
        );
        expect_offense(
            READER,
            "--- Doc.\nx = 1 function foo() end\n",
            2,
            7,
            message,
        );
    }

    #[test]
    fn local_functions_are_optional() {
        let src = "local function helper()\nend\n";
        expect_no_offenses(READER, src);
        let offenses = inspect_with(READER, src, "RequireForLocalFunctions: true");
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Missing documentation comment for `helper`."
        );
        assert_eq!((offenses[0].line, offenses[0].col), (1, 1));
        assert!(
            inspect_with(
                READER,
                "--- Helps.\nlocal function helper() end\n",
                "RequireForLocalFunctions: true"
            )
            .is_empty()
        );
    }

    #[test]
    fn anonymous_functions_are_exempt() {
        expect_no_offenses(
            READER,
            "local f = function() end\nhook.Add('A', 'b', function(x) end)\n",
        );
    }

    #[test]
    fn never_autocorrects() {
        let src = "function foo() end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
