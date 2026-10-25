//! `Lint/NonLiteralAssertMessage`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{ArgumentStyle, Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;

/// Flags `assert(condition, message)` calls whose message has to be computed: a concatenation,
/// a call or any other expression that is not a literal, a variable name, a field or an index.
///
/// Lua evaluates every argument before the call, so the message is built each time the line
/// runs, even when the assertion passes. Disabled by default; files that do not parse are
/// skipped.
pub struct NonLiteralAssertMessage;

impl Reader for NonLiteralAssertMessage {
    fn name(&self) -> &'static str {
        "Lint/NonLiteralAssertMessage"
    }

    fn description(&self) -> &'static str {
        "Checks for assert messages that are built even when the assertion passes."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut messages = Messages {
            source,
            found: Vec::new(),
        };
        walk(&block, &mut messages);
        for message in messages.found {
            if !cheap(message) {
                ctx.add_offense(
                    message.range.clone(),
                    "The message of `assert` is built even when the assertion passes; use a \
                     literal, or `if !condition then error(message) end`.",
                );
            }
        }
    }
}

/// Collects the message arguments of `assert` calls.
struct Messages<'s, 't> {
    source: &'s Source,
    found: Vec<&'t Expression>,
}

impl<'t> Visitor<'t> for Messages<'_, 't> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Call { callee, arguments } = &expression.kind else {
            return;
        };
        if let ExpressionKind::Name(name) = callee.kind
            && self.source.text_of(&name) == "assert"
            && arguments.style == ArgumentStyle::Parenthesized
            && let Some(message) = arguments.values.get(1)
        {
            self.found.push(message);
        }
    }
}

/// Whether `message` costs nothing to evaluate: a literal, a name, a field or an index.
fn cheap(message: &Expression) -> bool {
    matches!(
        message.unparenthesized().kind,
        ExpressionKind::Nil
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Vararg
            | ExpressionKind::Name(_)
            | ExpressionKind::Field { .. }
            | ExpressionKind::Index { .. }
    )
}

inventory::submit! { Registration(|| Box::new(NonLiteralAssertMessage)) }

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::Config;
    use crate::testing::*;

    const READER: &str = "Lint/NonLiteralAssertMessage";
    const MESSAGE: &str = "The message of `assert` is built even when the assertion passes; use \
                           a literal, or `if !condition then error(message) end`.";

    #[test]
    fn flags_computed_messages() {
        expect_offenses(
            READER,
            "assert(x, 'bad: '..y)\nlocal v = assert(f(), string.format('%s', y))\n\
             assert(x, ('%s'):format(y))\nassert(x, tostring(y))\n",
            &[
                (1, 11, MESSAGE),
                (2, 23, MESSAGE),
                (3, 11, MESSAGE),
                (4, 11, MESSAGE),
            ],
        );
    }

    #[test]
    fn accepts_cheap_messages() {
        expect_no_offenses(
            READER,
            "assert(x)\nassert(x, 'bad')\nassert(x, [[bad]])\nassert(x, message)\n\
             assert(x, ERRORS.bad)\nassert(x, errors[1])\nassert(x, ('bad'))\n\
             foo.assert(x, 'a'..b)\nverify(x, 'a'..b)\n",
        );
    }

    #[test]
    fn checks_lumen_templates() {
        expect_offense_lumen(
            READER,
            "return <view>{assert(x, 'a'..b)}</view>\n",
            1,
            25,
            MESSAGE,
        );
    }

    #[test]
    fn is_disabled_by_default() {
        let config = Config::defaults(Path::new("."));
        assert!(!config.reader(READER).enabled);
    }
}
