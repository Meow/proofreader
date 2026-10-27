//! `Security/NetMessageExecution`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::offense::Severity;
use crate::reader::{Context, Reader, Registration, yaml_strings};
use crate::readers::lint::ast::{
    Expression, ExpressionKind, FieldKey, Statement, StatementKind, Visitor, parse, walk,
};
use crate::readers::lint::scopes::{Resolution, resolve};
use crate::readers::security::realm::{Handler, server_handlers};
use crate::source::Source;
use crate::token::{Token, TokenKind};

/// The `net.Read*` functions whose result a client fully controls; the numeric, boolean,
/// entity, vector, angle and color readers are left out, as their values cannot carry code or
/// a command.
const DEFAULT_SOURCES: &[&str] = &[
    "net.ReadString",
    "net.ReadData",
    "net.ReadTable",
    "net.ReadType",
];

/// The functions that run code or commands, or write or remove files.
const DEFAULT_SINKS: &[&str] = &[
    "RunString",
    "RunStringEx",
    "CompileString",
    "game.ConsoleCommand",
    "RunConsoleCommand",
    "file.Write",
    "file.Append",
    "file.Delete",
    "file.Rename",
];

/// The net libraries' functions registering a handler that is called with the sending player
/// followed by the message's values (`netstream.Hook('name', function(ply, data) end)`).
const DEFAULT_PAYLOAD_RECEIVERS: &[&str] = &["netstream.Hook", "cable.receive", "Cable.receive"];

/// Functions whose result carries nothing of their arguments but a number, a boolean or a type
/// name.
const CLEANSING_FUNCTIONS: &[&str] = &[
    "tonumber",
    "tobool",
    "type",
    "IsValid",
    "isstring",
    "isnumber",
    "istable",
    "isbool",
    "isfunction",
];

/// Flags a call of one of the `Sinks` (`RunString`, `game.ConsoleCommand`, `file.Write`, ...)
/// inside a `net.Receive` handler that may run on the server when one of its arguments holds
/// data read from the message: a call of one of the `Sources` (`net.ReadString`,
/// `net.ReadData`, `net.ReadTable`, `net.ReadType`; the numeric, boolean and entity readers are
/// left out), or a variable or field assigned anywhere in the handler from an expression
/// holding such data (through `..`, `string.format` and other calls, table constructors,
/// fields and loops over tainted tables). Comparisons, `not`, `#`, unary minus and
/// `tonumber`, `tobool`, `type`, `IsValid` and the `is*` predicates do not pass the data on.
///
/// Handlers registered with one of the `PayloadReceivers` (`netstream.Hook`, Flux's and
/// Catwork's `cable.receive`), which are called with the sending player followed by the
/// message's values, are checked too, with every parameter after the first (and `...`) holding
/// message data.
///
/// The check stays inside the handler (the function literal passed to `net.Receive` or a
/// function of the file passed by name, closures nested in it included). Handlers registered in
/// client-only code are exempt, as for `Security/NetReadTable`. Files that do not parse are
/// skipped.
pub struct NetMessageExecution;

impl Reader for NetMessageExecution {
    fn name(&self) -> &'static str {
        "Security/NetMessageExecution"
    }

    fn description(&self) -> &'static str {
        "Checks for data from a net message reaching code execution, console commands or file writes on the server."
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![
            ("Sources", yaml_strings(DEFAULT_SOURCES)),
            ("Sinks", yaml_strings(DEFAULT_SINKS)),
            ("PayloadReceivers", yaml_strings(DEFAULT_PAYLOAD_RECEIVERS)),
        ]
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut handlers: Vec<(Handler, bool)> = server_handlers(&block, source, "net.Receive")
            .into_iter()
            .map(|handler| (handler, false))
            .collect();
        for receiver in ctx.option_str_list("PayloadReceivers", DEFAULT_PAYLOAD_RECEIVERS) {
            for handler in server_handlers(&block, source, &receiver) {
                handlers.push((handler, true));
            }
        }
        if handlers.is_empty() {
            return;
        }
        let Some(resolution) = resolve(source) else {
            return;
        };
        let declared: HashMap<usize, usize> = resolution
            .variables
            .iter()
            .enumerate()
            .map(|(id, variable)| (variable.token.start, id))
            .collect();
        let sources = ctx.option_str_list("Sources", DEFAULT_SOURCES);
        let sinks = ctx.option_str_list("Sinks", DEFAULT_SINKS);
        let mut found = BTreeSet::new();
        let binder = Binder {
            source,
            resolution: &resolution,
            declared: &declared,
        };
        for (handler, payload) in handlers {
            let mut flows = Flows {
                binder,
                flows: Vec::new(),
            };
            walk(&handler.function.body, &mut flows);
            let parameters = handler.function.parameters.iter().skip(1);
            let mut taint = Taint {
                binder,
                sources: &sources,
                tainted: parameters
                    .filter(|_| payload)
                    .map(|parameter| (binder.root(parameter), String::new()))
                    .collect(),
                vararg: payload && handler.function.vararg.is_some(),
            };
            taint.spread(&flows.flows);
            let mut finder = SinkFinder {
                taint: &taint,
                sinks: &sinks,
                found: Vec::new(),
            };
            walk(&handler.function.body, &mut finder);
            for (range, sink) in finder.found {
                found.insert((range.start, range.end, sink));
            }
        }
        for (start, end, sink) in found {
            ctx.add_offense(
                start..end,
                format!(
                    "Data from a net message reaches `{sink}`; {}",
                    consequence(&sink)
                ),
            );
        }
    }
}

/// What a client can do when its data reaches `sink`, as the end of a message.
fn consequence(sink: &str) -> &'static str {
    if sink.contains("RunString") || sink.contains("CompileString") {
        "a client can run any code on the server."
    } else if sink.contains("ConsoleCommand") {
        "a client can run console commands on the server."
    } else if sink.starts_with("file.") {
        "a client can choose what the server writes or removes."
    } else {
        "a client controls what it does on the server."
    }
}

/// What a variable or field is: the variable at its root and the dotted path of literal field
/// names below it (empty for the variable itself).
type Key = (Root, String);

/// The variable at the root of a [`Key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Root {
    /// A local, by its index in [`Resolution::variables`].
    Local(usize),
    /// A global, by name.
    Global(String),
}

/// Binds names to variables.
#[derive(Clone, Copy)]
struct Binder<'a> {
    source: &'a Source,
    resolution: &'a Resolution<'a>,
    declared: &'a HashMap<usize, usize>,
}

impl Binder<'_> {
    /// The variable the name `token` declares or refers to.
    fn root(&self, token: &Token) -> Root {
        if let Some(id) = self.declared.get(&token.start) {
            return Root::Local(*id);
        }
        match self
            .resolution
            .reference_at(token.start)
            .and_then(|reference| reference.variable)
        {
            Some(id) => Root::Local(id),
            None => Root::Global(self.source.text_of(token).to_owned()),
        }
    }

    /// The key of a name, a field with a literal name (`a.b`, `a['b']`), or, for any other
    /// index, of the indexed table.
    fn key(&self, expression: &Expression) -> Option<Key> {
        match &expression.kind {
            ExpressionKind::Name(token) => Some((self.root(token), String::new())),
            ExpressionKind::Parenthesized(inner) => self.key(inner),
            ExpressionKind::Field { object, name } => {
                let (root, path) = self.key(object)?;
                Some((root, format!("{path}.{}", self.source.text_of(name))))
            }
            ExpressionKind::Index { object, key } => {
                let (root, path) = self.key(object)?;
                match key.string_contents(self.source) {
                    Some(name) => Some((root, format!("{path}.{name}"))),
                    None => Some((root, path)),
                }
            }
            _ => None,
        }
    }
}

/// Collects the assignments of a handler as `(assigned key, value)` pairs: `local`
/// statements, assignments and the variables of generic `for` loops.
struct Flows<'a, 't> {
    binder: Binder<'a>,
    flows: Vec<(Key, &'t Expression)>,
}

impl<'t> Flows<'_, 't> {
    /// Records that each of `keys` is assigned the matching value of `values`; keys past the
    /// values take the last value when it can produce several.
    fn assign(&mut self, keys: Vec<Option<Key>>, values: &'t [Expression]) {
        for (index, key) in keys.into_iter().enumerate() {
            let value = values
                .get(index)
                .or_else(|| values.last().filter(|last| last.is_multi_valued()));
            if let (Some(key), Some(value)) = (key, value) {
                self.flows.push((key, value));
            }
        }
    }
}

impl<'t> Visitor<'t> for Flows<'_, 't> {
    fn statement(&mut self, statement: &'t Statement) {
        let binder = self.binder;
        match &statement.kind {
            StatementKind::Local { names, values, .. } => {
                let keys = names
                    .iter()
                    .map(|name| Some((binder.root(name), String::new())))
                    .collect();
                self.assign(keys, values);
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                let keys = targets.iter().map(|target| binder.key(target)).collect();
                self.assign(keys, values);
            }
            StatementKind::GenericFor {
                variables, values, ..
            } => {
                for variable in variables {
                    for value in values {
                        self.flows
                            .push(((binder.root(variable), String::new()), value));
                    }
                }
            }
            _ => {}
        }
    }
}

/// The variables and fields of a handler that hold data read from the message.
struct Taint<'a> {
    binder: Binder<'a>,
    sources: &'a [String],
    tainted: HashSet<Key>,
    /// Whether `...` holds the message's values.
    vararg: bool,
}

impl Taint<'_> {
    /// Taints every key assigned a tainted value, until nothing changes.
    fn spread(&mut self, flows: &[(Key, &Expression)]) {
        loop {
            let mut changed = false;
            for (key, value) in flows {
                if !self.tainted.contains(key) && self.is_tainted(value) {
                    self.tainted.insert(key.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Whether `key` names a tainted variable or field.
    fn is_tainted_key(&self, expression: &Expression) -> bool {
        self.binder
            .key(expression)
            .is_some_and(|key| self.tainted.contains(&key))
    }

    /// Whether the value of `expression` may hold data read from the message.
    fn is_tainted(&self, expression: &Expression) -> bool {
        match &expression.kind {
            ExpressionKind::Name(_) => self.is_tainted_key(expression),
            ExpressionKind::Parenthesized(inner) => self.is_tainted(inner),
            ExpressionKind::Field { object, .. } => {
                self.is_tainted_key(expression) || self.is_tainted(object)
            }
            ExpressionKind::Index { object, .. } => {
                self.is_tainted_key(expression) || self.is_tainted(object)
            }
            ExpressionKind::Call { callee, arguments } => {
                if let Some(function) = callee.dotted_name(self.binder.source) {
                    if self.sources.contains(&function) {
                        return true;
                    }
                    if CLEANSING_FUNCTIONS.contains(&function.as_str()) {
                        return false;
                    }
                }
                self.is_tainted(callee) || arguments.values.iter().any(|a| self.is_tainted(a))
            }
            ExpressionKind::MethodCall {
                object, arguments, ..
            } => self.is_tainted(object) || arguments.values.iter().any(|a| self.is_tainted(a)),
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                !matches!(
                    operator.kind,
                    TokenKind::Eq
                        | TokenKind::Ne
                        | TokenKind::Lt
                        | TokenKind::Le
                        | TokenKind::Gt
                        | TokenKind::Ge
                ) && (self.is_tainted(left) || self.is_tainted(right))
            }
            ExpressionKind::Table(table) => table.fields.iter().any(|field| {
                self.is_tainted(&field.value)
                    || matches!(&field.key, FieldKey::Bracketed(key) if self.is_tainted(key))
            }),
            ExpressionKind::Markup(splices) => splices.iter().any(|splice| self.is_tainted(splice)),
            ExpressionKind::Vararg => self.vararg,
            ExpressionKind::Nil
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Function(_)
            | ExpressionKind::Opaque
            | ExpressionKind::Unary { .. } => false,
        }
    }
}

/// Collects the sink calls with a tainted argument, by the range of the callee.
struct SinkFinder<'a> {
    taint: &'a Taint<'a>,
    sinks: &'a [String],
    found: Vec<(Range<usize>, String)>,
}

impl<'t> Visitor<'t> for SinkFinder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        let ExpressionKind::Call { callee, arguments } = &expression.kind else {
            return;
        };
        let Some(function) = callee.dotted_name(self.taint.binder.source) else {
            return;
        };
        if self.sinks.contains(&function)
            && arguments
                .values
                .iter()
                .any(|argument| self.taint.is_tainted(argument))
        {
            self.found.push((callee.range.clone(), function));
        }
    }
}

inventory::submit! { Registration(|| Box::new(NetMessageExecution)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Security/NetMessageExecution";

    const CODE: &str =
        "Data from a net message reaches `RunString`; a client can run any code on the server.";

    #[test]
    fn flags_message_data_reaching_sinks() {
        expect_offenses(
            READER,
            "net.Receive('a', function(len, ply)\n  RunString(net.ReadString())\n\
             game.ConsoleCommand(net.ReadString() .. '\\n')\n\
             file.Write('x.txt', net.ReadData(16))\nend)\n",
            &[
                (2, 3, CODE),
                (
                    3,
                    1,
                    "Data from a net message reaches `game.ConsoleCommand`; a client can run console commands on the server.",
                ),
                (
                    4,
                    1,
                    "Data from a net message reaches `file.Write`; a client can choose what the server writes or removes.",
                ),
            ],
        );
    }

    #[test]
    fn follows_variables_through_the_handler() {
        expect_offenses(
            READER,
            "net.Receive('a', function(len, ply)\n  local code = net.ReadString()\n\
             local wrapped = string.format('return %s', code:Trim())\n\
             local parts = { prefix, wrapped }\n\
             timer.Simple(0, function() RunString(table.concat(parts)) end)\nend)\n",
            &[(5, 28, CODE)],
        );
        expect_offense(
            READER,
            "net.Receive('a', function()\n  local data = {}\n  data.code = net.ReadString()\n\
             RunString(data.code)\nend)\n",
            4,
            1,
            CODE,
        );
        expect_offense(
            READER,
            "net.Receive('a', function()\n  local t = net.ReadTable()\n\
             for _, line in ipairs(t) do RunString(line) end\nend)\n",
            3,
            29,
            CODE,
        );
        expect_offense(
            READER,
            "net.Receive('a', function()\n  local code\n  if x then code = net.ReadString() end\n\
             RunString(code)\nend)\n",
            4,
            1,
            CODE,
        );
    }

    #[test]
    fn flags_named_handlers() {
        expect_offense(
            READER,
            "local function run(len, ply)\n  RunString(net.ReadString())\nend\nnet.Receive('a', run)\n",
            2,
            3,
            CODE,
        );
    }

    #[test]
    fn flags_payload_receivers() {
        expect_offenses(
            READER,
            "netstream.Hook('a', function(ply, data)\n  RunString(data.code)\n\
             RunConsoleCommand('kick', ply:Name())\nend)\n\
             Cable.receive('b', function(actor, ...)\n  local args = { ... }\n\
             game.ConsoleCommand(args[1])\nend)\n\
             if CLIENT then cable.receive('c', function(code) RunString(code) end) end\n",
            &[
                (2, 3, CODE),
                (
                    7,
                    1,
                    "Data from a net message reaches `game.ConsoleCommand`; a client can run console commands on the server.",
                ),
            ],
        );
    }

    #[test]
    fn accepts_harmless_values() {
        expect_no_offenses(
            READER,
            "net.Receive('a', function(len, ply)\n  local n = net.ReadUInt(8)\n\
             local s = net.ReadString()\n  local other = { name = 'x' }\n\
             RunConsoleCommand('bot_zombie', n)\n  RunConsoleCommand('x', tonumber(s) or 0)\n\
             RunConsoleCommand('y', s == 'on' and 1 or 0)\n  RunString(other.name)\n\
             RunConsoleCommand('z', #s)\n  ply.name = s\n\
             RunConsoleCommand('kick', ply:Name())\nend)\n",
        );
    }

    #[test]
    fn locals_are_told_apart() {
        expect_no_offenses(
            READER,
            "net.Receive('a', function()\n  do local s = net.ReadString() end\n\
             local s = 'say hi'\n  game.ConsoleCommand(s)\nend)\n",
        );
    }

    #[test]
    fn accepts_client_handlers_and_code_outside_handlers() {
        expect_no_offenses(
            READER,
            "if CLIENT then\n  net.Receive('a', function() RunString(net.ReadString()) end)\nend\n\
             RunString(net.ReadString())\n",
        );
    }

    #[test]
    fn options() {
        let src = "net.Receive('a', function()\n  local n = net.ReadUInt(8)\n  Exec(n)\n\
                   RunString(n)\nend)\n";
        let offenses = inspect_with(READER, src, "Sources: [net.ReadUInt]\nSinks: [Exec]");
        assert_eq!(offenses.len(), 1);
        assert_eq!(
            offenses[0].message,
            "Data from a net message reaches `Exec`; a client controls what it does on the server."
        );
        assert_eq!(offenses[0].severity, crate::offense::Severity::Error);
    }
}
