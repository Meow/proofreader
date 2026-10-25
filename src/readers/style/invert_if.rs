//! `Style/InvertIf`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Block, Function, Statement, StatementKind, Visitor, parse, walk};
use crate::token::Token;

/// Suggests inverting an `if` whose `else` branch only leaves early, to reduce nesting.
///
/// The `if` must have an `else` and no `elseif`, its `else` branch must be a single `return`
/// with at most one value (or `break` or `continue` inside a loop of the same function), its
/// `if` branch must not end by leaving itself, hold at least `MinBodyLength` statements, and be
/// followed by more code in the same block:
///
/// ```lua
/// if valid then
///   ...
/// else
///   return
/// end
/// more()
/// ```
///
/// reads better as `if !valid then return end` followed by the body. Disabled by default; never
/// autocorrected. Files that do not parse are skipped.
pub struct InvertIf;

impl Reader for InvertIf {
    fn name(&self) -> &'static str {
        "Style/InvertIf"
    }

    fn description(&self) -> &'static str {
        "Checks for `if` statements that can be inverted into a guard clause to reduce nesting."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("MinBodyLength", Yaml::Integer(3))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let Ok(chunk) = parse(ctx.source) else {
            return;
        };
        let mut finder = Finder {
            min_body: ctx.option_usize("MinBodyLength", 3),
            loop_bodies: Vec::new(),
            in_loop: Vec::new(),
            function_body_next: false,
            found: Vec::new(),
        };
        walk(&chunk, &mut finder);
        for keyword in finder.found {
            ctx.add_offense(
                keyword.range(),
                "Invert this `if` into a guard clause to reduce nesting.",
            );
        }
    }
}

/// Finds the invertible `if` statements, tracking whether each block is inside a loop.
struct Finder<'t> {
    min_body: usize,
    /// The bodies of the loop statements seen so far.
    loop_bodies: Vec<&'t Block>,
    /// For every open block, whether it is inside a loop of its function.
    in_loop: Vec<bool>,
    /// Whether the next block entered is the body of a function.
    function_body_next: bool,
    found: Vec<Token>,
}

impl<'t> Visitor<'t> for Finder<'t> {
    fn enter_function(&mut self, _function: &'t Function) {
        self.function_body_next = true;
    }

    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::While { body, .. }
            | StatementKind::Repeat { body, .. }
            | StatementKind::NumericFor { body, .. }
            | StatementKind::GenericFor { body, .. } => self.loop_bodies.push(body),
            _ => {}
        }
    }

    fn enter_block(&mut self, block: &'t Block) {
        let in_loop = if std::mem::take(&mut self.function_body_next) {
            false
        } else {
            self.loop_bodies
                .iter()
                .any(|body| std::ptr::eq(*body, block))
                || self.in_loop.last().copied().unwrap_or(false)
        };
        self.in_loop.push(in_loop);
        for (index, statement) in block.statements.iter().enumerate() {
            let followed = block.statements[index + 1..]
                .iter()
                .any(|next| next.kind != StatementKind::Empty);
            if followed && let Some(keyword) = self.invertible(statement, in_loop) {
                self.found.push(keyword);
            }
        }
    }

    fn leave_block(&mut self, _block: &'t Block) {
        self.in_loop.pop();
    }
}

impl Finder<'_> {
    /// The `if` keyword of `statement` when it is an invertible `if`.
    fn invertible(&self, statement: &Statement, in_loop: bool) -> Option<Token> {
        let StatementKind::If {
            clauses,
            otherwise: Some((_, otherwise)),
            ..
        } = &statement.kind
        else {
            return None;
        };
        let [clause] = clauses.as_slice() else {
            return None;
        };
        let exits = meaningful(otherwise);
        let [exit] = exits.as_slice() else {
            return None;
        };
        let early = match &exit.kind {
            StatementKind::Return { values, .. } => values.len() <= 1,
            StatementKind::Break | StatementKind::Continue => in_loop,
            _ => false,
        };
        let body = meaningful(&clause.body);
        let ends_with_exit = body.last().is_some_and(|last| {
            matches!(
                last.kind,
                StatementKind::Return { .. } | StatementKind::Break | StatementKind::Continue
            )
        });
        (early && !ends_with_exit && body.len() >= self.min_body).then_some(clause.keyword)
    }
}

/// The statements of `block` other than `;`.
fn meaningful(block: &Block) -> Vec<&Statement> {
    block
        .statements
        .iter()
        .filter(|statement| statement.kind != StatementKind::Empty)
        .collect()
}

inventory::submit! { Registration(|| Box::new(InvertIf)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/InvertIf";
    const MESSAGE: &str = "Invert this `if` into a guard clause to reduce nesting.";

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn flags_early_return_in_else() {
        expect_offense(
            READER,
            "local function f(x)\n  if x then\n    a()\n    b()\n    c()\n  else\n    return\n  \
             end\n\n  d()\nend\n",
            2,
            3,
            MESSAGE,
        );
        expect_offense(
            READER,
            "local function f(x)\n  if x then a() b() c() else return nil end\n  d()\nend\n",
            2,
            3,
            MESSAGE,
        );
    }

    #[test]
    fn flags_break_and_continue_inside_loops_only() {
        expect_offense(
            READER,
            "for _, v in ipairs(t) do\n  if v then a() b() c() else continue end\n  d()\nend\n",
            2,
            3,
            MESSAGE,
        );
        expect_offense(
            READER,
            "while x do\n  do\n    if v then a() b() c() else break end\n    d()\n  end\nend\n",
            3,
            5,
            MESSAGE,
        );
        expect_no_offenses(
            READER,
            "while x do\n  f(function()\n    if v then a() b() c() else break end\n    d()\n  \
             end)\nend\n",
        );
    }

    #[test]
    fn accepts_other_shapes() {
        let body = "a()\n  b()\n  c()";
        for src in [
            format!("if x then\n  {body}\nelse\n  return\nend\n"),
            format!("if x then\n  {body}\nelse\n  return 1, 2\nend\nd()\n"),
            format!("if x then\n  {body}\nelseif y then\n  e()\nelse\n  return\nend\nd()\n"),
            format!("if x then\n  {body}\n  return\nelse\n  return\nend\nd()\n"),
            format!("if x then\n  {body}\nelse\n  e()\n  return\nend\nd()\n"),
            format!("if x then\n  {body}\nend\nd()\n"),
            "if x then\n  a()\n  b()\nelse\n  return\nend\nd()\n".to_owned(),
        ] {
            expect_no_offenses(READER, &src);
        }
    }

    #[test]
    fn honours_min_body_length() {
        let src = "if x then a() else return end\nd()\n";
        expect_no_offenses(READER, src);
        assert_eq!(inspect_with(READER, src, "MinBodyLength: 1").len(), 1);
    }

    #[test]
    fn never_autocorrects() {
        let src = "if x then a() b() c() else return end\nd()\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
