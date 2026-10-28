//! `Lint/UnbalancedAssignment`.

use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{ExpressionKind, Statement, StatementKind, Visitor, parse, walk};
use crate::source::Source;

/// Flags assignments whose numbers of targets and values differ.
///
/// In `local a, b = 1` and `a, b = 1` the targets without a value are assigned `nil`, unless the
/// last value is a call or `...`, which can produce several values. A last value of `nil`
/// (`local a, b = nil`) states the intent and is accepted. Values beyond the number of targets
/// (`local a = 1, 2`) are evaluated and discarded. `local a, b` without values is fine. Files
/// that do not parse are skipped.
pub struct UnbalancedAssignment;

impl Reader for UnbalancedAssignment {
    fn name(&self) -> &'static str {
        "Lint/UnbalancedAssignment"
    }

    fn description(&self) -> &'static str {
        "Checks for assignments with more targets than values or more values than targets."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(chunk) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            found: Vec::new(),
        };
        walk(&chunk, &mut finder);
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the unbalanced targets and values.
struct Finder<'a> {
    source: &'a Source,
    found: Vec<(Range<usize>, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        let (targets, values): (Vec<Range<usize>>, _) = match &statement.kind {
            StatementKind::Local { names, values, .. } if !values.is_empty() => {
                (names.iter().map(|name| name.range()).collect(), values)
            }
            StatementKind::Assignment {
                targets, values, ..
            } => (
                targets.iter().map(|target| target.range.clone()).collect(),
                values,
            ),
            _ => return,
        };
        let counts = format!("[{}/{}]", values.len(), targets.len());
        if values.len() > targets.len() {
            for value in &values[targets.len()..] {
                self.found.push((
                    value.range.clone(),
                    format!("Value is discarded: there are more values than targets. {counts}"),
                ));
            }
            return;
        }
        let Some(last) = values.last() else {
            return;
        };
        if values.len() == targets.len()
            || last.is_multi_valued()
            || last.kind == ExpressionKind::Nil
        {
            return;
        }
        for target in &targets[values.len()..] {
            let text = self.source.text.get(target.clone()).unwrap_or_default();
            self.found.push((
                target.clone(),
                format!(
                    "`{text}` is assigned `nil`: there are fewer values than targets. {counts}"
                ),
            ));
        }
    }
}

inventory::submit! { Registration(|| Box::new(UnbalancedAssignment)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/UnbalancedAssignment";

    #[test]
    fn flags_targets_without_values() {
        expect_offense(
            READER,
            "local a, b = 1\n",
            1,
            10,
            "`b` is assigned `nil`: there are fewer values than targets. [1/2]",
        );
        expect_offenses(
            READER,
            "x, t.y, t[1] = 0\n",
            &[
                (
                    1,
                    4,
                    "`t.y` is assigned `nil`: there are fewer values than targets. [1/3]",
                ),
                (
                    1,
                    9,
                    "`t[1]` is assigned `nil`: there are fewer values than targets. [1/3]",
                ),
            ],
        );
    }

    #[test]
    fn understands_the_extended_syntax() {
        expect_no_offenses(
            READER,
            "local a, b = x?.f()\nlocal c, d = o?.:m()\nx += 1\nconst e, g = f()\n",
        );
        expect_offense(
            READER,
            "const a, b = 1\n",
            1,
            10,
            "`b` is assigned `nil`: there are fewer values than targets. [1/2]",
        );
    }

    #[test]
    fn flags_discarded_values() {
        expect_offense(
            READER,
            "local a = 1, 2\n",
            1,
            14,
            "Value is discarded: there are more values than targets. [2/1]",
        );
        expect_offense(
            READER,
            "a = f(), g()\n",
            1,
            10,
            "Value is discarded: there are more values than targets. [2/1]",
        );
    }

    #[test]
    fn accepts_balanced_and_multi_valued_assignments() {
        expect_no_offenses(
            READER,
            "local a, b = 1, 2\nlocal c, d = f()\nlocal e, g = 1, t:m()\n\
             local h, i = ...\nlocal j, k\nl, m = n, o\nlocal p, q = nil\n",
        );
        expect_no_offenses(READER, "local function f(...) local a, b = ... end\n");
    }

    #[test]
    fn parenthesized_calls_produce_one_value() {
        expect_offense(
            READER,
            "local a, b = (f())\n",
            1,
            10,
            "`b` is assigned `nil`: there are fewer values than targets. [1/2]",
        );
    }

    #[test]
    fn checks_nested_functions_and_lumen() {
        expect_offense_lumen(
            READER,
            "return <view>{f(function() local a, b = 1 end)}</view>\n",
            1,
            37,
            "`b` is assigned `nil`: there are fewer values than targets. [1/2]",
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "local a, b = 1\nf(\n");
    }
}
