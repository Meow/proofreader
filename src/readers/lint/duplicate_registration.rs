//! `Lint/DuplicateRegistration`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{parse, walk};
use crate::readers::lint::duplicate_require::VisibleCalls;

/// The registration functions, the kind of name each registers and the noun used in messages.
const FUNCTIONS: [(&str, &str, &str); 5] = [
    ("util.AddNetworkString", "net", "Network string"),
    ("net.Receive", "receiver", "Network receiver for"),
    ("concommand.Add", "concommand", "Console command"),
    ("CreateConVar", "convar", "Console variable"),
    ("CreateClientConVar", "convar", "Console variable"),
];

/// Flags a Garry's Mod name registered a second time in the same file: a network string
/// (`util.AddNetworkString`), a network receiver (`net.Receive`), a console command
/// (`concommand.Add`) or a console variable (`CreateConVar`, `CreateClientConVar`).
///
/// The second registration silently replaces or repeats the first. Names are string literals,
/// compared case-insensitively as the game does. As with `Lint/DuplicateRequire`, only an
/// earlier registration in the same or an enclosing block counts, so registrations in the
/// `SERVER` and `CLIENT` branches of an `if` do not conflict. Files that do not parse are
/// skipped.
pub struct DuplicateRegistration;

impl Reader for DuplicateRegistration {
    fn name(&self) -> &'static str {
        "Lint/DuplicateRegistration"
    }

    fn description(&self) -> &'static str {
        "Checks for network strings, receivers, console commands and variables registered twice."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = VisibleCalls::with_key(
            source,
            |name: &str| FUNCTIONS.iter().any(|(function, _, _)| *function == name),
            |function, argument| format!("{}:{}", kind(function).0, argument.to_lowercase()),
        );
        walk(&block, &mut finder);
        for duplicate in finder.duplicates {
            ctx.add_offense(
                duplicate.range,
                format!(
                    "{} `{}` is already registered on line {}.",
                    kind(&duplicate.function).1,
                    duplicate.key,
                    duplicate.first_line
                ),
            );
        }
    }
}

/// The kind and message noun of the registration function `function`.
fn kind(function: &str) -> (&'static str, &'static str) {
    FUNCTIONS
        .iter()
        .find(|(name, _, _)| *name == function)
        .map_or(("", ""), |(_, kind, noun)| (kind, noun))
}

inventory::submit! { Registration(|| Box::new(DuplicateRegistration)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/DuplicateRegistration";

    #[test]
    fn flags_repeated_registrations() {
        expect_offenses(
            READER,
            "util.AddNetworkString('flux_a')\nutil.AddNetworkString('Flux_A')\n\
             concommand.Add('x', f)\nconcommand.Add('x', g)\n\
             CreateConVar('v', '1')\nCreateClientConVar('v', '1')\n\
             net.Receive('m', f)\nnet.Receive('m', g)\n",
            &[
                (
                    2,
                    1,
                    "Network string `Flux_A` is already registered on line 1.",
                ),
                (4, 1, "Console command `x` is already registered on line 3."),
                (
                    6,
                    1,
                    "Console variable `v` is already registered on line 5.",
                ),
                (
                    8,
                    1,
                    "Network receiver for `m` is already registered on line 7.",
                ),
            ],
        );
    }

    #[test]
    fn kinds_do_not_mix() {
        expect_no_offenses(
            READER,
            "util.AddNetworkString('a')\nnet.Receive('a', f)\nconcommand.Add('a', f)\n\
             CreateConVar('a', '1')\n",
        );
    }

    #[test]
    fn accepts_realm_branches_and_dynamic_names() {
        expect_no_offenses(
            READER,
            "if SERVER then\n  net.Receive('a', f)\nelse\n  net.Receive('a', g)\nend\n\
             for _, name in ipairs(t) do util.AddNetworkString(name) end\n\
             util.AddNetworkString(prefix..'a')\nutil.AddNetworkString(prefix..'a')\n",
        );
    }

    #[test]
    fn flags_registrations_repeated_in_a_nested_block() {
        expect_offense(
            READER,
            "util.AddNetworkString('a')\nif SERVER then\n  util.AddNetworkString('a')\nend\n",
            3,
            3,
            "Network string `a` is already registered on line 1.",
        );
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(
            READER,
            "util.AddNetworkString('a')\nutil.AddNetworkString('a')\nif then\n",
        );
    }
}
