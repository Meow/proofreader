//! `Lint/UnreachableCode`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Block, Statement, StatementKind, Visitor, parse, walk};
use crate::source::Source;

/// Flags statements that can never run because a statement before them in the same block always
/// leaves it.
///
/// `break`, `continue`, `goto`, `return` and a call statement of one of `TerminatingFunctions`
/// (`error(...)`) always leave the block, and so do a `do ... end` whose body always leaves it and
/// an `if` with an `else` all of whose branches always leave it. A `::label::` can be jumped to,
/// so the code after it is reachable again; `;` statements are ignored. Each unreachable run is
/// reported once, at its first statement. Files that do not parse are skipped.
pub struct UnreachableCode;

impl Reader for UnreachableCode {
    fn name(&self) -> &'static str {
        "Lint/UnreachableCode"
    }

    fn description(&self) -> &'static str {
        "Checks for statements that can never run."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![(
            "TerminatingFunctions",
            Yaml::Array(vec![Yaml::String("error".to_owned())]),
        )]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(chunk) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            terminating: ctx.option_str_list("TerminatingFunctions", &["error"]),
            found: Vec::new(),
        };
        walk(&chunk, &mut finder);
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the first statement of every unreachable run.
struct Finder<'a> {
    source: &'a Source,
    terminating: Vec<String>,
    found: Vec<(std::ops::Range<usize>, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn enter_block(&mut self, block: &'t Block) {
        let mut exit: Option<&Statement> = None;
        let mut reported = false;
        for statement in &block.statements {
            match (&statement.kind, exit) {
                (StatementKind::Label { .. }, _) => {
                    exit = None;
                    reported = false;
                }
                (StatementKind::Empty, _) => {}
                (_, Some(leaving)) => {
                    if !reported {
                        reported = true;
                        self.found
                            .push((statement.range.clone(), self.message(leaving)));
                    }
                }
                (_, None) => {
                    if self.leaves(statement) {
                        exit = Some(statement);
                    }
                }
            }
        }
    }
}

impl Finder<'_> {
    /// Whether `statement` always leaves the block it is in.
    fn leaves(&self, statement: &Statement) -> bool {
        match &statement.kind {
            StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Goto { .. }
            | StatementKind::Return { .. } => true,
            StatementKind::Call(call) => call
                .as_call(self.source)
                .is_some_and(|(name, _)| self.terminating.contains(&name)),
            StatementKind::Do { body } => self.always_leaves(body),
            StatementKind::If {
                clauses, otherwise, ..
            } => {
                otherwise
                    .as_ref()
                    .is_some_and(|(_, body)| self.always_leaves(body))
                    && clauses
                        .iter()
                        .all(|clause| self.always_leaves(&clause.body))
            }
            _ => false,
        }
    }

    /// Whether the end of `block` can never be reached: a statement that leaves it is not
    /// followed by a label.
    fn always_leaves(&self, block: &Block) -> bool {
        let mut left = false;
        for statement in &block.statements {
            if matches!(statement.kind, StatementKind::Label { .. }) {
                left = false;
            } else if !left && self.leaves(statement) {
                left = true;
            }
        }
        left
    }

    /// The message for code made unreachable by `leaving`.
    fn message(&self, leaving: &Statement) -> String {
        let (line, _) = self.source.position(leaving.range.start);
        match &leaving.kind {
            StatementKind::If { .. } => {
                format!(
                    "Unreachable code; every branch of the `if` on line {line} leaves the block."
                )
            }
            StatementKind::Do { .. } => {
                format!("Unreachable code; the `do` block on line {line} always leaves the block.")
            }
            StatementKind::Call(call) => {
                let name = call
                    .as_call(self.source)
                    .map_or_else(String::new, |(name, _)| name);
                format!("Unreachable code after the `{name}` call on line {line}.")
            }
            StatementKind::Goto { .. } => format!("Unreachable code after `goto` on line {line}."),
            StatementKind::Continue => format!("Unreachable code after `continue` on line {line}."),
            StatementKind::Return { .. } => {
                format!("Unreachable code after `return` on line {line}.")
            }
            _ => format!("Unreachable code after `break` on line {line}."),
        }
    }
}

inventory::submit! { Registration(|| Box::new(UnreachableCode)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/UnreachableCode";

    #[test]
    fn flags_code_after_jumps() {
        expect_offense(
            READER,
            "for i = 1, 2 do\n  break\n  print(i)\nend\n",
            3,
            3,
            "Unreachable code after `break` on line 2.",
        );
        expect_offense(
            READER,
            "for i = 1, 2 do\n  continue\n  print(i)\nend\n",
            3,
            3,
            "Unreachable code after `continue` on line 2.",
        );
        expect_offense(
            READER,
            "goto done\nprint(1)\n::done::\n",
            2,
            1,
            "Unreachable code after `goto` on line 1.",
        );
    }

    #[test]
    fn looks_inside_short_function_blocks() {
        expect_offense(
            READER,
            "f(|| -> do\n  error('x')\n  print(2)\nend)\n",
            3,
            3,
            "Unreachable code after the `error` call on line 2.",
        );
        expect_no_offenses(READER, "f(|x| -> x, || -> error('x'))\nprint(1)\n");
    }

    #[test]
    fn flags_code_after_terminating_calls() {
        expect_offense(
            READER,
            "local function f(x)\n  error('bad')\n  return x\nend\n",
            3,
            3,
            "Unreachable code after the `error` call on line 2.",
        );
        expect_no_offenses(
            READER,
            "local function f(x)\n  assert(x)\n  return x\nend\n",
        );
        expect_no_offenses(
            READER,
            "local function f(x)\n  x.error('a')\n  return x\nend\n",
        );
        let offenses = inspect_with(
            READER,
            "Error('x')\nprint(1)\n",
            "TerminatingFunctions: [error, Error]",
        );
        assert_eq!(offenses.len(), 1);
    }

    #[test]
    fn flags_code_after_blocks_that_always_leave() {
        expect_offense(
            READER,
            "while x do\n  if a then\n    break\n  else\n    continue\n  end\n  f()\nend\n",
            7,
            3,
            "Unreachable code; every branch of the `if` on line 2 leaves the block.",
        );
        expect_offense(
            READER,
            "while x do\n  do\n    break\n  end\n  f()\nend\n",
            5,
            3,
            "Unreachable code; the `do` block on line 2 always leaves the block.",
        );
        expect_offense(
            READER,
            "local function f()\n  do return 1 end\n  g()\nend\n",
            3,
            3,
            "Unreachable code; the `do` block on line 2 always leaves the block.",
        );
    }

    #[test]
    fn accepts_branches_that_may_fall_through() {
        expect_no_offenses(
            READER,
            "while x do\n  if a then break end\n  f()\nend\n\
             while x do\n  if a then break elseif b then g() else break end\n  f()\nend\n\
             while x do\n  do if a then break end end\n  f()\nend\n",
        );
        expect_no_offenses(READER, "while x do\n  while y do break end\n  f()\nend\n");
    }

    #[test]
    fn labels_make_code_reachable_again() {
        expect_no_offenses(READER, "goto skip\n::skip::\nprint(1)\n");
        expect_no_offenses(
            READER,
            "while x do\n  do goto next ::next:: end\n  f()\nend\n",
        );
        expect_offenses(
            READER,
            "goto a\nf()\n::a::\ngoto b\ng()\n::b::\n",
            &[
                (2, 1, "Unreachable code after `goto` on line 1."),
                (5, 1, "Unreachable code after `goto` on line 4."),
            ],
        );
    }

    #[test]
    fn reports_each_run_once_and_ignores_semicolons() {
        expect_offense(
            READER,
            "while x do\n  break;\n  f()\n  g()\nend\n",
            3,
            3,
            "Unreachable code after `break` on line 2.",
        );
        expect_no_offenses(READER, "while x do\n  break;\nend\n");
    }

    #[test]
    fn checks_nested_functions_and_lumen_splices() {
        expect_offense(
            READER,
            "hook.Add('Think', 'x', function()\n  for _ = 1, 2 do break f() end\nend)\n",
            2,
            25,
            "Unreachable code after `break` on line 2.",
        );
        expect_offense_lumen(
            READER,
            "return <view>{map(t, function(v)\n  error('x')\n  return v\nend)}</view>\n",
            3,
            3,
            "Unreachable code after the `error` call on line 2.",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "while x do\n  break\n  f(\nend\n");
    }
}
