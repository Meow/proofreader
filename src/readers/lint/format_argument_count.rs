//! `Lint/FormatArgumentCount`.

use std::ops::Range;

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{Expression, ExpressionKind, Visitor, parse, walk};
use crate::source::Source;

/// The formatting functions checked by default: Lua's and GMod's global alias.
const FUNCTIONS: [&str; 2] = ["string.format", "Format"];

/// Flags `string.format` calls whose number of arguments does not match the conversions of
/// their literal format string.
///
/// The calls are those of the functions in `Functions` (`string.format(fmt, ...)`,
/// `Format(fmt, ...)`) and the method form on a string literal (`('%d'):format(n)`), when the
/// format is a string literal. Every `%` conversion
/// (`%[-+ #0]*[0-9]*(.[0-9]*)?[cdiouxXeEfgGqsaA]`) takes one argument and `%%` none; a format
/// with any other `%` sequence, or with an escape that could spell a `%`, is skipped. Too few
/// arguments raise a "bad argument" error at run time; extra arguments are ignored. When the
/// last argument is a call or `...`, which can produce any number of values, only the
/// arguments before it are counted and only too many of those are flagged. Files that do not
/// parse are skipped.
pub struct FormatArgumentCount;

impl Reader for FormatArgumentCount {
    fn name(&self) -> &'static str {
        "Lint/FormatArgumentCount"
    }

    fn description(&self) -> &'static str {
        "Checks that format calls pass as many arguments as their format string expects."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("Functions", yaml_strings(&FUNCTIONS))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let functions = ctx.option_str_list("Functions", &FUNCTIONS);
        let source = ctx.source;
        let Ok(chunk) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            functions: &functions,
            found: Vec::new(),
        };
        walk(&chunk, &mut finder);
        for (range, message) in finder.found {
            ctx.add_offense(range, message);
        }
    }
}

/// Collects the mismatched format calls.
struct Finder<'a> {
    source: &'a Source,
    functions: &'a [String],
    found: Vec<(Range<usize>, String)>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let (callee, format, arguments) = match &expression.kind {
            ExpressionKind::Call { callee, arguments } => {
                let Some(name) = callee.dotted_name(self.source) else {
                    return;
                };
                if !self.functions.contains(&name) {
                    return;
                }
                let Some((format, rest)) = arguments.values.split_first() else {
                    return;
                };
                (callee.range.clone(), format, rest)
            }
            ExpressionKind::MethodCall {
                object,
                name,
                arguments,
            } if self.source.text_of(name) == "format" => (
                name.range(),
                object.unparenthesized(),
                &arguments.values[..],
            ),
            _ => return,
        };
        let Some(expected) = format.string_contents(self.source).and_then(conversions) else {
            return;
        };
        let open = arguments.last().is_some_and(Expression::is_multi_valued);
        let given = arguments.len() - usize::from(open);
        let message = if given > expected {
            let extra = if given - expected == 1 {
                "the extra argument is ignored"
            } else {
                "the extra arguments are ignored"
            };
            let at_least = if open { "at least " } else { "" };
            format!(
                "Format string expects {} but gets {at_least}{given}; {extra}.",
                count(expected)
            )
        } else if given < expected && !open {
            format!(
                "Format string expects {} but gets {given}.",
                count(expected)
            )
        } else {
            return;
        };
        self.found.push((callee, message));
    }
}

/// `n argument` or `n arguments`.
fn count(n: usize) -> String {
    if n == 1 {
        "1 argument".to_owned()
    } else {
        format!("{n} arguments")
    }
}

/// The number of arguments the format string `format` (as written between its quotes) takes,
/// or `None` when it holds a malformed conversion or an escape that could spell a `%`.
fn conversions(format: &str) -> Option<usize> {
    let numeric_escape = format.split('\\').skip(1).any(|after| {
        after
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == 'x' || c == 'u')
    });
    if numeric_escape {
        return None;
    }
    let bytes = format.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if bytes.get(i) == Some(&b'%') {
            i += 1;
            continue;
        }
        while bytes
            .get(i)
            .is_some_and(|b| matches!(b, b'-' | b'+' | b' ' | b'#' | b'0'))
        {
            i += 1;
        }
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'.') {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        if !bytes
            .get(i)
            .is_some_and(|b| b"cdiouxXeEfgGqsaA".contains(b))
        {
            return None;
        }
        i += 1;
        count += 1;
    }
    Some(count)
}

inventory::submit! { Registration(|| Box::new(FormatArgumentCount)) }

#[cfg(test)]
mod tests {
    use super::conversions;
    use crate::testing::*;

    const READER: &str = "Lint/FormatArgumentCount";

    #[test]
    fn counts_conversions() {
        assert_eq!(conversions("plain"), Some(0));
        assert_eq!(conversions("%d%% of %s"), Some(2));
        assert_eq!(conversions("%-5.2f|%+03d|%#x|% i|%.3s|%5s|%q|%c"), Some(8));
        assert_eq!(conversions("100%"), None);
        assert_eq!(conversions("%y"), None);
        assert_eq!(conversions("%*d"), None);
        assert_eq!(conversions("\\37d %s"), None);
        assert_eq!(conversions("\\x25d %s"), None);
        assert_eq!(conversions("a\\n%s"), Some(1));
    }

    #[test]
    fn flags_too_few_arguments() {
        expect_offense(
            READER,
            "print(string.format('%s has %d of %d', name, n))\n",
            1,
            7,
            "Format string expects 3 arguments but gets 2.",
        );
        expect_offense(
            READER,
            "local s = Format(\"%s\")\n",
            1,
            11,
            "Format string expects 1 argument but gets 0.",
        );
        expect_offense(
            READER,
            "local s = ('%d/%d'):format(a)\n",
            1,
            21,
            "Format string expects 2 arguments but gets 1.",
        );
        expect_offense(
            READER,
            "local s = Format [[%s]]\n",
            1,
            11,
            "Format string expects 1 argument but gets 0.",
        );
    }

    #[test]
    fn flags_too_many_arguments() {
        expect_offense(
            READER,
            "local s = string.format('%d', a, b)\n",
            1,
            11,
            "Format string expects 1 argument but gets 2; the extra argument is ignored.",
        );
        expect_offense(
            READER,
            "local s = ('100%%'):format(a, b, f())\n",
            1,
            21,
            "Format string expects 0 arguments but gets at least 2; the extra arguments are \
             ignored.",
        );
    }

    #[test]
    fn accepts_matching_and_open_argument_lists() {
        expect_no_offenses(
            READER,
            "local a = string.format('%s: %5.1f%%', name, value)\n\
             local b = string.format('%s %s %s', unpack(t))\n\
             local c = Format('%s %s', x, ...)\n\
             local d = string.format(fmt, a)\n\
             local e = fmt:format(a, b)\n\
             local f = ('%s'):rep(3)\n\
             local g = string.format('%s', x, f())\n\
             local h = string.format('%y', a)\n\
             local i = string.format('%s' .. suffix, a)\n",
        );
    }

    #[test]
    fn reads_the_functions_option() {
        let src = "local s = sformat('%d')\nlocal t = Format('%d')\n";
        let offenses = inspect_with(READER, src, "Functions: [sformat]");
        let found: Vec<(u32, u32)> = offenses
            .iter()
            .map(|offense| (offense.line, offense.col))
            .collect();
        assert_eq!(found, [(1, 11)]);
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "string.format('%d')\nf(\n");
    }
}
