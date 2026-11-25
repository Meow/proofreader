//! `Lint/InvalidDirective`.

use crate::directive::problems;
use crate::reader::{Context, Reader, Registration};

/// Flags inline directives that do nothing they seem to: a comment starting with
/// `proofreader-` whose action is misspelled (`-- proofreader-disable-lines`,
/// `-- proofreader-disable-line: Style/Not`), and reader names in a directive's list that name
/// no reader or department (`-- proofreader-disable-line Style/Nott`). Each comes with the
/// nearest valid spelling when there is one. See [`crate::directive`].
pub struct InvalidDirective;

impl Reader for InvalidDirective {
    fn name(&self) -> &'static str {
        "Lint/InvalidDirective"
    }

    fn description(&self) -> &'static str {
        "Checks for misspelled inline directives and directives naming unknown readers."
    }

    fn investigate(&self, ctx: &mut Context) {
        for (range, message) in problems(ctx.source) {
            ctx.add_offense(range, message);
        }
    }
}

inventory::submit! { Registration(|| Box::new(InvalidDirective)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/InvalidDirective";

    #[test]
    fn flags_misspelled_directives_and_unknown_readers() {
        expect_offenses(
            READER,
            "x = 1 -- proofreader-disable-line Style/Nott\n\
             -- proofreader-disable-next-line: Style/Not\ny = 2\n",
            &[
                (
                    1,
                    35,
                    "Unknown reader or department `Style/Nott` in a directive; did you mean \
                     Style/Not?",
                ),
                (
                    2,
                    4,
                    "Malformed directive `proofreader-disable-next-line:`; did you mean \
                     proofreader-disable-next-line?",
                ),
            ],
        );
    }

    #[test]
    fn accepts_valid_directives_and_other_comments() {
        expect_no_offenses(
            READER,
            "x = 1 -- proofreader-disable-line Style/Not, Layout -- generated\n\
             -- proofreader-disable\n-- proofreader-enable\n-- see proofreader-disable-line\n\
             -- proofreader disable-line\n",
        );
    }
}
