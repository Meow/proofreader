//! `Lint/ConstReassignment`.

use crate::offense::Severity;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::scopes::{Variable, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::source::Source;

/// Flags what LuaJIT 3.0 rejects about `const` locals when it compiles a file.
///
/// A constant (`const x = ...`, `const function f()`) cannot be assigned again: not by a plain
/// or compound assignment (`x = 2`, `x += 1`, one target of `a, x = 1, 2`), not by a
/// `function x()` statement, and not from a nested function ("attempt to assign to const
/// variable"). Its name cannot be declared again while the constant is visible either, not even
/// in a nested block or function: a `local`, another `const`, a `local function`, a parameter
/// (of a short function too, and the implicit `self` of a method) or a loop variable ("attempt
/// to re-declare const variable"). A `const` may shadow an earlier `local`, and a name is free
/// again once the block of its constant has ended. Files that do not parse are skipped.
pub struct ConstReassignment;

impl Reader for ConstReassignment {
    fn name(&self) -> &'static str {
        "Lint/ConstReassignment"
    }

    fn description(&self) -> &'static str {
        "Checks for assignments to `const` locals and declarations of a visible constant's name."
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Some(resolution) = resolve(source) else {
            return;
        };
        let variables = &resolution.variables;
        let constants: Vec<&Variable> = variables
            .iter()
            .filter(|variable| is_constant(source, variable))
            .collect();
        if constants.is_empty() {
            return;
        }
        let mut found = Vec::new();
        for constant in &constants {
            let (line, _) = source.position(constant.token.start);
            for write in &constant.writes {
                found.push((
                    write.range(),
                    format!(
                        "Attempt to assign to const variable `{}` (declared on line {line}).",
                        constant.name
                    ),
                ));
            }
        }
        for variable in variables {
            let at = variable.token.start;
            let Some(constant) = constants.iter().find(|constant| {
                constant.name == variable.name
                    && constant.token.end <= at
                    && constant.scope.start <= at
                    && at < constant.scope.end
            }) else {
                continue;
            };
            let (line, _) = source.position(constant.token.start);
            found.push((
                variable.token.range(),
                format!(
                    "Attempt to re-declare const variable `{}` (declared on line {line}).",
                    variable.name
                ),
            ));
        }
        found.sort_by_key(|(range, _)| range.start);
        for (range, message) in found {
            ctx.add_offense(range, message);
        }
    }
}

/// Whether `variable` is declared by `const` rather than `local`: the declaring construct of a
/// local starts at its keyword.
fn is_constant(source: &Source, variable: &Variable) -> bool {
    matches!(
        variable.kind,
        DeclarationKind::Local | DeclarationKind::LocalFunction
    ) && !variable.implicit
        && source
            .text
            .get(variable.scope.start..)
            .is_some_and(|rest| rest.starts_with("const"))
}

inventory::submit! { Registration(|| Box::new(ConstReassignment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/ConstReassignment";

    #[test]
    fn flags_assignments_to_constants() {
        expect_offenses(
            READER,
            "const x = 1\nx = 2\nx += 3\nlocal y\ny, x = 1, 2\n\
             const function f() end\nfunction f() end\n\
             local function g()\n  x = 4\nend\n",
            &[
                (
                    2,
                    1,
                    "Attempt to assign to const variable `x` (declared on line 1).",
                ),
                (
                    3,
                    1,
                    "Attempt to assign to const variable `x` (declared on line 1).",
                ),
                (
                    5,
                    4,
                    "Attempt to assign to const variable `x` (declared on line 1).",
                ),
                (
                    7,
                    10,
                    "Attempt to assign to const variable `f` (declared on line 6).",
                ),
                (
                    9,
                    3,
                    "Attempt to assign to const variable `x` (declared on line 1).",
                ),
            ],
        );
    }

    #[test]
    fn flags_every_target_of_a_multiple_constant() {
        expect_offense(
            READER,
            "const a, b = 1\nb = 2\n",
            2,
            1,
            "Attempt to assign to const variable `b` (declared on line 1).",
        );
    }

    #[test]
    fn flags_declarations_of_a_visible_constant_name() {
        expect_offenses(
            READER,
            "const a = 1\nlocal a = 2\nconst a = 3\nlocal function a() end\n\
             do\n  for a = 1, 2 do end\n  for _, a in pairs(t) do end\nend\n\
             local f = function(a) end\nlocal g = |a| -> a\n\
             local function h()\n  local a = 4\nend\n",
            &[
                (
                    2,
                    7,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    3,
                    7,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    4,
                    16,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    6,
                    7,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    7,
                    10,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    9,
                    20,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    10,
                    12,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
                (
                    12,
                    9,
                    "Attempt to re-declare const variable `a` (declared on line 1).",
                ),
            ],
        );
    }

    #[test]
    fn flags_a_name_repeated_in_one_constant_declaration() {
        expect_offense(
            READER,
            "const a, a = 1, 2\n",
            1,
            10,
            "Attempt to re-declare const variable `a` (declared on line 1).",
        );
    }

    #[test]
    fn flags_the_implicit_self_of_a_method() {
        expect_offense(
            READER,
            "const self = 1\nfunction t:m() end\n",
            2,
            12,
            "Attempt to re-declare const variable `self` (declared on line 1).",
        );
    }

    #[test]
    fn accepts_shadowed_locals_ended_scopes_fields_and_mutation() {
        expect_no_offenses(
            READER,
            "local a = 1\nconst a = 2\n\
             do\n  const b = 1\nend\nlocal b = 2\nb = 3\n\
             local function f()\n  const c = 1\nend\nlocal c = 2\n\
             const t = {}\nt.x = 1\nt[1] = 2\nfunction t.y() end\nfunction t:z() end\n\
             repeat\n  const d = 1\nuntil d == 1\nlocal d = 2\n",
        );
    }

    #[test]
    fn accepts_locals_without_constants() {
        expect_no_offenses(READER, "local x = 1\nx = 2\nlocal x = 3\n");
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(READER, "const x = 1\nx = 2\nif then\n");
    }
}
