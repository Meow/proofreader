//! `Lint/NetMessageMismatch`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    Block, Expression, ExpressionKind, FieldKey, Function, Statement, StatementKind, Visitor,
    parse, walk,
};
use crate::readers::security::realm::{
    Realm, file_realm, named_functions, realm_ranges, registrations,
};
use crate::source::Source;
use crate::token::TokenKind;

/// The payload functions of the `net` library: the name, the wire format a write pairs with a
/// read by, whether it writes, and the position of its bit count argument.
const PAYLOAD: [(&str, &str, bool, Option<usize>); 36] = [
    ("net.WriteAngle", "angle", true, None),
    ("net.ReadAngle", "angle", false, None),
    ("net.WriteBit", "bit", true, None),
    ("net.ReadBit", "bit", false, None),
    ("net.WriteBool", "bool", true, None),
    ("net.ReadBool", "bool", false, None),
    ("net.WriteColor", "color", true, None),
    ("net.ReadColor", "color", false, None),
    ("net.WriteData", "data", true, None),
    ("net.ReadData", "data", false, None),
    ("net.WriteDouble", "double", true, None),
    ("net.ReadDouble", "double", false, None),
    ("net.WriteEntity", "entity", true, None),
    ("net.ReadEntity", "entity", false, None),
    ("net.WriteFloat", "float", true, None),
    ("net.ReadFloat", "float", false, None),
    ("net.WriteMatrix", "matrix", true, None),
    ("net.ReadMatrix", "matrix", false, None),
    ("net.WriteNormal", "normal", true, None),
    ("net.ReadNormal", "normal", false, None),
    ("net.WritePlayer", "player", true, None),
    ("net.ReadPlayer", "player", false, None),
    ("net.WriteString", "string", true, None),
    ("net.ReadString", "string", false, None),
    ("net.WriteTable", "table", true, None),
    ("net.ReadTable", "table", false, None),
    ("net.WriteType", "type", true, None),
    ("net.ReadType", "type", false, None),
    ("net.WriteUInt64", "uint64", true, None),
    ("net.ReadUInt64", "uint64", false, None),
    ("net.WriteVector", "vector", true, None),
    ("net.ReadVector", "vector", false, None),
    ("net.WriteInt", "int", true, Some(1)),
    ("net.ReadInt", "int", false, Some(0)),
    ("net.WriteUInt", "uint", true, Some(1)),
    ("net.ReadUInt", "uint", false, Some(0)),
];

/// The functions ending a message, with the realm the message arrives in.
const SENDS: [(&str, Realm); 6] = [
    ("net.Send", Realm::Client),
    ("net.SendOmit", Realm::Client),
    ("net.SendPAS", Realm::Client),
    ("net.SendPVS", Realm::Client),
    ("net.Broadcast", Realm::Client),
    ("net.SendToServer", Realm::Server),
];

/// Functions of the `net` library besides the sends that neither write nor read a payload.
const NEUTRAL: [&str; 4] = [
    "net.Start",
    "net.Receive",
    "net.BytesWritten",
    "net.BytesLeft",
];

/// Flags a net message whose `net.Read*` calls in a `net.Receive` handler cannot match the
/// `net.Write*` calls of a `net.Start` ... send in the same file: a different number of values,
/// a value read with another function than it is written with (in another order, or of another
/// type), or an integer read with another literal bit count than it is written with.
///
/// The writes of a message are the payload calls of the statements after a `net.Start` with a
/// literal name, up to the send (`net.Send`, `net.Broadcast`, `net.SendToServer`, ...) in the
/// same block; the reads are those of the handler, a function literal or a function the file
/// defines once under the name it passes. Functions nested in either are not part of it. A
/// payload call inside a branch or loop (or in the right operand of `and`, `or` and `??`, a
/// branch of a ternary, or after `?.`) may run any number of times and matches any number of
/// values of its kind, as in GLuaLS. A message is compared only when the sender and the
/// receiver run in opposite realms the file makes certain (a `cl_`/`sv_` file or a `client`/
/// `server` directory, or a `CLIENT`/`SERVER` branch or guard) and the send goes to the
/// receiver's realm, and only with senders whose payload is known: a call of a `net` function
/// that is no payload function, of a function whose name contains `Write` or `Read`, of a
/// parameter, or of a function of the file that uses the net library leaves the payload
/// unknown. A receiver matching any of the senders of its message is not reported; otherwise
/// the first mismatch with the closest sender is. Names are compared case-insensitively.
pub struct NetMessageMismatch;

impl Reader for NetMessageMismatch {
    fn name(&self) -> &'static str {
        "Lint/NetMessageMismatch"
    }

    fn description(&self) -> &'static str {
        "Checks that the reads of a net message in a file match the writes of its sender there."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let file = FileNet::new(&block, source);
        let sends = file.sends(&block);
        if sends.is_empty() {
            return;
        }
        for receive in file.receives(&block) {
            for (range, message) in check(source, &receive, &sends) {
                ctx.add_offense(range, message);
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(NetMessageMismatch)) }

/// One payload call.
#[derive(Debug, Clone)]
struct Op {
    /// The function called, such as `net.WriteString`.
    function: &'static str,
    /// The wire format a write and a read pair by.
    format: &'static str,
    /// The call.
    range: Range<usize>,
    /// Whether the call may run any number of times.
    dynamic: bool,
    /// The literal bit count, for the integer functions.
    bits: Option<u32>,
}

impl Op {
    /// Whether this write is read by `read`.
    fn pairs_with(&self, read: &Op) -> bool {
        self.format == read.format
    }
}

/// A `net.Start` ... send transaction with a known payload.
#[derive(Debug)]
struct Send {
    /// The message name, lowercased.
    key: String,
    /// The `net.Start` call.
    start: Range<usize>,
    /// The writes, in order.
    writes: Vec<Op>,
    /// The realm the message arrives in.
    destination: Realm,
    /// The realm the sender runs in, when certain.
    realm: Option<Realm>,
}

/// A `net.Receive` registration with a known handler.
#[derive(Debug)]
struct Receive {
    /// The message name as written.
    name: String,
    /// The message name, lowercased.
    key: String,
    /// The message name argument.
    name_range: Range<usize>,
    /// The reads, in order.
    reads: Vec<Op>,
    /// The realm the receiver runs in, when certain.
    realm: Option<Realm>,
}

/// What a file says about net messages: its functions, the parameters in scope and its realms.
struct FileNet<'s, 't> {
    source: &'s Source,
    /// The functions defined under a name.
    functions: HashMap<String, Vec<&'t Function>>,
    /// Every function with its parameter names.
    scopes: Vec<(Range<usize>, Vec<&'s str>)>,
    /// Whether a function (by the offset of its keyword) uses the net library.
    touches: RefCell<HashMap<usize, bool>>,
    /// The realm of the file.
    file_realm: Option<Realm>,
    /// The client-only and server-only ranges.
    client: Vec<Range<usize>>,
    server: Vec<Range<usize>>,
}

impl<'s, 't> FileNet<'s, 't> {
    fn new(block: &'t Block, source: &'s Source) -> Self {
        let mut scopes = Scopes {
            source,
            scopes: Vec::new(),
        };
        walk(block, &mut scopes);
        FileNet {
            source,
            functions: named_functions(block, source),
            scopes: scopes.scopes,
            touches: RefCell::new(HashMap::new()),
            file_realm: file_realm(source),
            client: realm_ranges(block, source, Realm::Client),
            server: realm_ranges(block, source, Realm::Server),
        }
    }

    /// The realm code at `offset` runs in, when the file makes it certain.
    fn realm_at(&self, offset: usize) -> Option<Realm> {
        let client = self.client.iter().any(|range| range.contains(&offset));
        let server = self.server.iter().any(|range| range.contains(&offset));
        match (self.file_realm, client, server) {
            (Some(Realm::Client), _, true) | (Some(Realm::Server), true, _) => None,
            (Some(realm), _, _) => Some(realm),
            (None, true, false) => Some(Realm::Client),
            (None, false, true) => Some(Realm::Server),
            _ => None,
        }
    }

    /// The parameter names of the functions enclosing `offset` (or starting at it).
    fn parameters_at(&self, offset: usize) -> HashSet<&'s str> {
        self.scopes
            .iter()
            .filter(|(range, _)| range.contains(&offset))
            .flat_map(|(_, names)| names.iter().copied())
            .collect()
    }

    /// Whether calling `function` may write or read a payload.
    fn touches_net(&self, function: &'t Function) -> bool {
        let key = function.keyword.start;
        if let Some(touches) = self.touches.borrow().get(&key) {
            return *touches;
        }
        self.touches.borrow_mut().insert(key, false);
        let mut payload = Payload::new(self, None, self.parameters_at(key));
        payload.block(&function.body, false);
        let touches = payload.opaque || !payload.ops.is_empty();
        self.touches.borrow_mut().insert(key, touches);
        touches
    }

    /// The transactions of the file whose payload is known.
    fn sends(&self, block: &'t Block) -> Vec<Send> {
        let mut finder = Starts { starts: Vec::new() };
        walk(block, &mut finder);
        let mut sends = Vec::new();
        for (statements, index) in finder.starts {
            let StatementKind::Call(start) = &statements[index].kind else {
                continue;
            };
            let Some(name) = message_name(start, self.source, "net.Start") else {
                continue;
            };
            let mut payload = Payload::new(self, Some(true), self.parameters_at(start.range.start));
            let mut destination = None;
            for statement in &statements[index + 1..] {
                if let StatementKind::Call(call) = &statement.kind
                    && let Some((function, _)) = call.as_call(self.source)
                {
                    if function == "net.Start" {
                        break;
                    }
                    if let Some((_, realm)) = SENDS.iter().find(|(send, _)| *send == function) {
                        destination = Some(*realm);
                        break;
                    }
                }
                payload.statement(statement, false);
            }
            if let (Some(destination), false) = (destination, payload.opaque) {
                sends.push(Send {
                    key: name.to_lowercase(),
                    start: start.range.clone(),
                    writes: payload.ops,
                    destination,
                    realm: self.realm_at(start.range.start),
                });
            }
        }
        sends
    }

    /// The receivers of the file whose handler and reads are known.
    fn receives(&self, block: &'t Block) -> Vec<Receive> {
        let mut receives = Vec::new();
        for registration in registrations(block, self.source, "net.Receive") {
            let ([function], Some(name)) = (
                registration.functions.as_slice(),
                message_name(registration.call, self.source, "net.Receive"),
            ) else {
                continue;
            };
            let mut payload = Payload::new(
                self,
                Some(false),
                self.parameters_at(function.keyword.start),
            );
            payload.block(&function.body, false);
            if payload.opaque {
                continue;
            }
            let Some((_, arguments)) = registration.call.as_call(self.source) else {
                continue;
            };
            receives.push(Receive {
                name: name.to_owned(),
                key: name.to_lowercase(),
                name_range: arguments.values[0].range.clone(),
                reads: payload.ops,
                realm: self.realm_at(registration.call.range.start),
            });
        }
        receives
    }
}

/// The literal message name of a call of `function` (`net.Start('name')`).
fn message_name<'a>(call: &Expression, source: &'a Source, function: &str) -> Option<&'a str> {
    let (name, arguments) = call.as_call(source)?;
    if name != function {
        return None;
    }
    arguments.values.first()?.string_contents(source)
}

/// Collects every function with its parameter names.
struct Scopes<'s> {
    source: &'s Source,
    scopes: Vec<(Range<usize>, Vec<&'s str>)>,
}

impl<'t> Visitor<'t> for Scopes<'_> {
    fn enter_function(&mut self, function: &'t Function) {
        let names = function
            .parameters
            .iter()
            .map(|token| self.source.text_of(token))
            .collect();
        self.scopes
            .push((function.keyword.start..function.end.end, names));
    }
}

/// Collects the `net.Start` call statements: their block's statements and their index there.
struct Starts<'t> {
    starts: Vec<(&'t [Statement], usize)>,
}

impl<'t> Visitor<'t> for Starts<'t> {
    fn enter_block(&mut self, block: &'t Block) {
        for (index, statement) in block.statements.iter().enumerate() {
            if matches!(statement.kind, StatementKind::Call(_)) {
                self.starts.push((&block.statements, index));
            }
        }
    }
}

/// Collects the payload calls of code in evaluation order, noting whether the payload is
/// unknown.
struct Payload<'f, 's, 't> {
    file: &'f FileNet<'s, 't>,
    /// Which calls to keep: writes, reads, or both (`None`).
    writes: Option<bool>,
    /// The parameter names in scope.
    parameters: HashSet<&'s str>,
    ops: Vec<Op>,
    opaque: bool,
}

impl<'f, 's, 't> Payload<'f, 's, 't> {
    fn new(file: &'f FileNet<'s, 't>, writes: Option<bool>, parameters: HashSet<&'s str>) -> Self {
        Payload {
            file,
            writes,
            parameters,
            ops: Vec::new(),
            opaque: false,
        }
    }

    fn block(&mut self, block: &'t Block, dynamic: bool) {
        for statement in &block.statements {
            self.statement(statement, dynamic);
        }
    }

    fn expressions(&mut self, expressions: &'t [Expression], dynamic: bool) {
        for expression in expressions {
            self.expression(expression, dynamic);
        }
    }

    fn statement(&mut self, statement: &'t Statement, dynamic: bool) {
        match &statement.kind {
            StatementKind::Empty
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Goto { .. }
            | StatementKind::Label { .. }
            | StatementKind::LocalFunction { .. }
            | StatementKind::Function { .. } => {}
            StatementKind::Local { values, .. } | StatementKind::Return { values, .. } => {
                self.expressions(values, dynamic);
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                self.expressions(targets, dynamic);
                self.expressions(values, dynamic);
            }
            StatementKind::CompoundAssignment { target, value, .. } => {
                self.expression(target, dynamic);
                self.expression(value, dynamic);
            }
            StatementKind::Call(call) => self.expression(call, dynamic),
            StatementKind::Do { body } => self.block(body, dynamic),
            StatementKind::While { condition, body } => {
                self.expression(condition, true);
                self.block(body, true);
            }
            StatementKind::Repeat { body, condition } => {
                self.block(body, true);
                self.expression(condition, true);
            }
            StatementKind::If {
                clauses, otherwise, ..
            } => {
                for (index, clause) in clauses.iter().enumerate() {
                    self.expression(&clause.condition, dynamic || index > 0);
                    self.block(&clause.body, true);
                }
                if let Some((_, body)) = otherwise {
                    self.block(body, true);
                }
            }
            StatementKind::NumericFor {
                start,
                limit,
                step,
                body,
                ..
            } => {
                self.expression(start, dynamic);
                self.expression(limit, dynamic);
                if let Some(step) = step {
                    self.expression(step, dynamic);
                }
                self.block(body, true);
            }
            StatementKind::GenericFor { values, body, .. } => {
                self.expressions(values, dynamic);
                self.block(body, true);
            }
        }
    }

    fn expression(&mut self, expression: &'t Expression, dynamic: bool) {
        match &expression.kind {
            ExpressionKind::Nil
            | ExpressionKind::True
            | ExpressionKind::False
            | ExpressionKind::Number
            | ExpressionKind::String(_)
            | ExpressionKind::Vararg
            | ExpressionKind::Opaque
            | ExpressionKind::Name(_)
            | ExpressionKind::Function(_) => {}
            ExpressionKind::Table(table) => {
                for field in &table.fields {
                    if let FieldKey::Bracketed(key) = &field.key {
                        self.expression(key, dynamic);
                    }
                    self.expression(&field.value, dynamic);
                }
            }
            ExpressionKind::Markup(splices) => self.expressions(splices, dynamic),
            ExpressionKind::Parenthesized(inner) => self.expression(inner, dynamic),
            ExpressionKind::Field { object, .. } => self.expression(object, dynamic),
            ExpressionKind::SafeField { object, .. } => self.expression(object, dynamic),
            ExpressionKind::Index { object, key } => {
                self.expression(object, dynamic);
                self.expression(key, dynamic);
            }
            ExpressionKind::SafeIndex { object, key } => {
                self.expression(object, dynamic);
                self.expression(key, true);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.expression(callee, dynamic);
                self.expressions(&arguments.values, dynamic);
                self.call(expression, dynamic);
            }
            ExpressionKind::SafeCall { callee, arguments } => {
                self.expression(callee, dynamic);
                self.expressions(&arguments.values, true);
                self.call(expression, true);
            }
            ExpressionKind::MethodCall {
                object,
                name,
                arguments,
            } => {
                self.expression(object, dynamic);
                self.expressions(&arguments.values, dynamic);
                self.method(self.file.source.text_of(name));
            }
            ExpressionKind::SafeMethodCall {
                object,
                name,
                arguments,
                ..
            } => {
                self.expression(object, dynamic);
                self.expressions(&arguments.values, true);
                self.method(self.file.source.text_of(name));
            }
            ExpressionKind::Ternary {
                condition,
                consequent,
                alternative,
                ..
            } => {
                self.expression(condition, dynamic);
                self.expression(consequent, true);
                self.expression(alternative, true);
            }
            ExpressionKind::Unary { operand, .. } => self.expression(operand, dynamic),
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left, dynamic);
                let short_circuit = matches!(
                    operator.kind,
                    TokenKind::And
                        | TokenKind::AndAnd
                        | TokenKind::Or
                        | TokenKind::OrOr
                        | TokenKind::Coalesce
                );
                self.expression(right, dynamic || short_circuit);
            }
        }
    }

    /// Handles a call of a plain function (`f(...)`, `a.b(...)`, `a?.b(...)`).
    fn call(&mut self, call: &'t Expression, dynamic: bool) {
        let source = self.file.source;
        let (callee, arguments) = match &call.kind {
            ExpressionKind::Call { callee, arguments }
            | ExpressionKind::SafeCall { callee, arguments } => (callee, arguments),
            _ => return,
        };
        let Some(name) = callee.dotted_name(source) else {
            self.opaque = true;
            return;
        };
        if let Some((function, format, write, bits)) =
            PAYLOAD.iter().find(|(function, ..)| *function == name)
        {
            if self.writes.is_none_or(|writes| writes == *write) {
                self.ops.push(Op {
                    function,
                    format,
                    range: call.range.clone(),
                    dynamic,
                    bits: bits
                        .and_then(|index| arguments.values.get(index))
                        .and_then(|argument| literal_bits(argument, source)),
                });
            }
            return;
        }
        if NEUTRAL.contains(&name.as_str()) || SENDS.iter().any(|(send, _)| *send == name) {
            return;
        }
        let last = name.rsplit('.').next().unwrap_or(&name);
        let root = name.split('.').next().unwrap_or(&name);
        if name.starts_with("net.")
            || mentions_payload(last)
            || self.parameters.contains(root)
            || self.file.functions.get(&name).is_some_and(|functions| {
                functions
                    .iter()
                    .any(|function| self.file.touches_net(function))
            })
        {
            self.opaque = true;
        }
    }

    /// Handles a call of the method `name`.
    fn method(&mut self, name: &str) {
        if mentions_payload(name)
            || self.file.functions.iter().any(|(path, functions)| {
                path.rsplit('.').next() == Some(name)
                    && functions
                        .iter()
                        .any(|function| self.file.touches_net(function))
            })
        {
            self.opaque = true;
        }
    }
}

/// Whether a function or method name suggests it writes or reads a payload (`WriteNetVars`,
/// `readState`).
fn mentions_payload(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("write") || lower.contains("read")
}

/// The value of an integer literal bit count.
fn literal_bits(argument: &Expression, source: &Source) -> Option<u32> {
    if argument.kind != ExpressionKind::Number {
        return None;
    }
    let text = source.text[argument.range.clone()].replace('_', "");
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Whether `writes` can be read by `reads`, a payload call that may run any number of times
/// standing for any number of values of its kind (as GLuaLS matches them).
fn can_match(writes: &[Op], reads: &[Op]) -> bool {
    if writes.len() == reads.len()
        && writes
            .iter()
            .zip(reads)
            .all(|(write, read)| write.pairs_with(read))
    {
        return true;
    }
    if !writes.iter().chain(reads).any(|op| op.dynamic) {
        return false;
    }
    let mut memo = vec![vec![None; reads.len() + 1]; writes.len() + 1];
    match_from(writes, reads, 0, 0, &mut memo)
}

/// Whether `writes[i..]` can be read by `reads[j..]`.
fn match_from(
    writes: &[Op],
    reads: &[Op],
    i: usize,
    j: usize,
    memo: &mut Vec<Vec<Option<bool>>>,
) -> bool {
    if let Some(known) = memo[i][j] {
        return known;
    }
    let result = if i == writes.len() {
        reads[j..].iter().all(|read| read.dynamic)
    } else if j == reads.len() {
        writes[i..].iter().all(|write| write.dynamic)
    } else {
        let (write, read) = (&writes[i], &reads[j]);
        let pairs = write.pairs_with(read);
        (write.dynamic && match_from(writes, reads, i + 1, j, memo))
            || (read.dynamic && match_from(writes, reads, i, j + 1, memo))
            || (pairs && match_from(writes, reads, i + 1, j + 1, memo))
            || (pairs && write.dynamic && match_from(writes, reads, i, j + 1, memo))
            || (pairs && read.dynamic && match_from(writes, reads, i + 1, j, memo))
    };
    memo[i][j] = Some(result);
    result
}

/// How many leading writes `reads` reads in order.
fn matching_prefix(writes: &[Op], reads: &[Op]) -> usize {
    writes
        .iter()
        .zip(reads)
        .take_while(|(write, read)| write.pairs_with(read))
        .count()
}

/// `count` followed by `word`, pluralised with an `s`.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// The offenses of `receive` against the senders of its message in the file.
fn check(source: &Source, receive: &Receive, sends: &[Send]) -> Vec<(Range<usize>, String)> {
    let Some(realm) = receive.realm else {
        return Vec::new();
    };
    let senders: Vec<&Send> = sends
        .iter()
        .filter(|send| {
            send.key == receive.key
                && send.destination == realm
                && send.realm == Some(realm.opposite())
        })
        .collect();
    if senders.is_empty() {
        return Vec::new();
    }
    let matching: Vec<&Send> = senders
        .iter()
        .copied()
        .filter(|send| can_match(&send.writes, &receive.reads))
        .collect();
    if matching.is_empty() {
        let closest = senders
            .iter()
            .copied()
            .max_by(|a, b| {
                let key = |send: &Send| {
                    (
                        matching_prefix(&send.writes, &receive.reads),
                        send.writes.len(),
                    )
                };
                key(a).cmp(&key(b)).then(b.start.start.cmp(&a.start.start))
            })
            .expect("senders");
        return vec![first_mismatch(source, receive, closest)];
    }
    let mut offenses = Vec::new();
    let mut reported = HashSet::new();
    for send in matching {
        for (index, (write, read)) in send.writes.iter().zip(&receive.reads).enumerate() {
            if write.dynamic || read.dynamic || !write.pairs_with(read) {
                break;
            }
            let (Some(written), Some(read_bits)) = (write.bits, read.bits) else {
                continue;
            };
            if written != read_bits && reported.insert((index, written, read_bits)) {
                offenses.push((
                    read.range.clone(),
                    format!(
                        "Value {} of net message `{}` is written with {} on line {} but read \
                         with {read_bits}.",
                        index + 1,
                        receive.name,
                        plural(written as usize, "bit"),
                        source.position(write.range.start).0,
                    ),
                ));
            }
        }
    }
    offenses
}

/// The first difference between what `send` writes and `receive` reads, which cannot match.
fn first_mismatch(source: &Source, receive: &Receive, send: &Send) -> (Range<usize>, String) {
    let (writes, reads) = (&send.writes, &receive.reads);
    let dynamic = writes.iter().chain(reads).any(|op| op.dynamic);
    if !dynamic && writes.len() != reads.len() {
        return (
            receive.name_range.clone(),
            format!(
                "Net message `{}` is sent with {} on line {} but read as {}.",
                receive.name,
                plural(writes.len(), "value"),
                source.position(send.start.start).0,
                plural(reads.len(), "value"),
            ),
        );
    }
    for (index, (write, read)) in writes.iter().zip(reads).enumerate() {
        if write.pairs_with(read) {
            continue;
        }
        let out_of_order = writes
            .iter()
            .enumerate()
            .any(|(other, write)| other != index && write.pairs_with(read));
        return (
            read.range.clone(),
            format!(
                "Value {} of net message `{}` is written with `{}` on line {} but read with \
                 `{}`{}.",
                index + 1,
                receive.name,
                write.function,
                source.position(write.range.start).0,
                read.function,
                if out_of_order { " (out of order)" } else { "" },
            ),
        );
    }
    (
        receive.name_range.clone(),
        format!(
            "Net message `{}` is read in a way that cannot match what it is sent with on line {}.",
            receive.name,
            source.position(send.start.start).0,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const READER: &str = "Lint/NetMessageMismatch";

    /// A shared file sending `body` from the server and receiving with `handler` on the client.
    fn server_to_client(writes: &str, reads: &str) -> String {
        format!(
            "if SERVER then\n  net.Start('msg')\n{writes}  net.Broadcast()\n\
             else\n  net.Receive('msg', function()\n{reads}  end)\nend\n"
        )
    }

    #[test]
    fn accepts_matching_payloads() {
        expect_no_offenses(
            READER,
            &server_to_client(
                "  net.WriteString(name)\n  net.WriteUInt(n, 8)\n  net.WriteEntity(e)\n",
                "    local name = net.ReadString()\n    local n = net.ReadUInt(8)\n\
                     \x20   local e = net.ReadEntity()\n",
            ),
        );
    }

    #[test]
    fn flags_a_type_mismatch() {
        expect_offense(
            READER,
            &server_to_client(
                "  net.WriteString(name)\n  net.WriteBool(b)\n",
                "    local name = net.ReadString()\n    local b = net.ReadEntity()\n",
            ),
            9,
            15,
            "Value 2 of net message `msg` is written with `net.WriteBool` on line 4 but read \
             with `net.ReadEntity`.",
        );
    }

    #[test]
    fn flags_an_order_mismatch() {
        expect_offense(
            READER,
            &server_to_client(
                "  net.WriteString(name)\n  net.WriteBool(b)\n",
                "    local b = net.ReadBool()\n    local name = net.ReadString()\n",
            ),
            8,
            15,
            "Value 1 of net message `msg` is written with `net.WriteString` on line 3 but read \
             with `net.ReadBool` (out of order).",
        );
    }

    #[test]
    fn flags_a_count_mismatch() {
        expect_offense(
            READER,
            &server_to_client(
                "  net.WriteString(name)\n  net.WriteBool(b)\n",
                "    local name = net.ReadString()\n",
            ),
            7,
            15,
            "Net message `msg` is sent with 2 values on line 2 but read as 1 value.",
        );
    }

    #[test]
    fn flags_a_bit_count_mismatch() {
        expect_offense(
            READER,
            &server_to_client(
                "  net.WriteUInt(n, 8)\n  net.WriteInt(m, 0x10)\n",
                "    local n = net.ReadUInt(16)\n    local m = net.ReadInt(16)\n",
            ),
            8,
            15,
            "Value 1 of net message `msg` is written with 8 bits on line 3 but read with 16.",
        );
    }

    #[test]
    fn ignores_bit_counts_that_are_not_literals() {
        expect_no_offenses(
            READER,
            &server_to_client(
                "  net.WriteUInt(n, BITS)\n",
                "    local n = net.ReadUInt(8)\n",
            ),
        );
    }

    #[test]
    fn conditional_and_repeated_values_match_any_number() {
        expect_no_offenses(
            READER,
            &server_to_client(
                "  net.WriteBool(x ~= nil)\n  if x then net.WriteString(x) end\n\
                 \x20 net.WriteUInt(#t, 8)\n  for _, v in ipairs(t) do net.WriteEntity(v) end\n",
                "    local x = net.ReadBool() and net.ReadString()\n\
                 \x20   for i = 1, net.ReadUInt(8) do list[i] = net.ReadEntity() end\n",
            ),
        );
        expect_offense(
            READER,
            &server_to_client(
                "  if x then net.WriteString(x) end\n",
                "    local x = net.ReadEntity()\n",
            ),
            7,
            15,
            "Value 1 of net message `msg` is written with `net.WriteString` on line 3 but read \
             with `net.ReadEntity`.",
        );
        expect_offense(
            READER,
            &server_to_client(
                "  net.WriteString(s)\n  net.WriteBool(b)\n",
                "    if x then net.ReadString() end\n",
            ),
            7,
            15,
            "Net message `msg` is read in a way that cannot match what it is sent with on line 2.",
        );
    }

    #[test]
    fn names_are_case_insensitive() {
        expect_offense(
            READER,
            "if SERVER then\n  net.Start('Flux_Msg')\n  net.WriteBool(b)\n  net.Send(ply)\n\
             else\n  net.Receive('flux_msg', function() net.ReadString() end)\nend\n",
            6,
            38,
            "Value 1 of net message `flux_msg` is written with `net.WriteBool` on line 3 but \
             read with `net.ReadString`.",
        );
    }

    #[test]
    fn named_handlers() {
        expect_offense(
            READER,
            "if CLIENT then\n  net.Start('a')\n  net.WriteBool(b)\n  net.SendToServer()\n\
             else\n  local function on_a(len, ply)\n    local s = net.ReadString()\n  end\n\
             \x20 net.Receive('a', on_a)\nend\n",
            7,
            15,
            "Value 1 of net message `a` is written with `net.WriteBool` on line 3 but read \
             with `net.ReadString`.",
        );
    }

    #[test]
    fn realms_must_be_opposite_and_match_the_send() {
        expect_no_offenses(
            READER,
            "net.Start('a')\nnet.WriteBool(b)\nnet.Broadcast()\n\
             net.Receive('a', function() net.ReadString() end)\n",
        );
        expect_no_offenses(
            READER,
            "if CLIENT then\n  net.Start('a')\n  net.WriteBool(b)\n  net.Broadcast()\n\
             else\n  net.Receive('a', function() net.ReadString() end)\nend\n",
        );
        expect_offense(
            READER,
            "if CLIENT then\n  net.Start('a')\n  net.WriteBool(b)\n  net.SendToServer()\n\
             end\nif SERVER then\n  net.Receive('a', function(len, ply) net.ReadString() end)\n\
             end\n",
            7,
            39,
            "Value 1 of net message `a` is written with `net.WriteBool` on line 3 but read \
             with `net.ReadString`.",
        );
    }

    #[test]
    fn any_matching_sender_is_enough() {
        expect_no_offenses(
            READER,
            "if SERVER then\n  net.Start('a')\n  net.WriteBool(b)\n  net.Broadcast()\n\
             \x20 net.Start('a')\n  net.WriteString(s)\n  net.Broadcast()\n\
             else\n  net.Receive('a', function() net.ReadString() end)\nend\n",
        );
    }

    #[test]
    fn unknown_payloads_are_skipped() {
        for (writes, reads) in [
            ("  write_payload(x)\n", "    net.ReadString()\n"),
            ("  self:WriteNetVars()\n", "    net.ReadString()\n"),
            ("  net.WriteCompressedTable(t)\n", "    net.ReadString()\n"),
            (
                "  net.WriteString(s)\n",
                "    net.ReadString()\n    helper()\n",
            ),
            (
                "  net.WriteString(s)\n",
                "    net.ReadString()\n    obj:Decode()\n",
            ),
            ("  net.WriteString(s)\n  t[k]()\n", "    net.ReadBool()\n"),
        ] {
            let src = format!(
                "local function helper() return net.ReadBool() end\n\
                 function META:Decode() return net.ReadBool() end\n{}",
                server_to_client(writes, reads)
            );
            expect_no_offenses(READER, &src);
        }
        expect_no_offenses(
            READER,
            "if SERVER then\n  function send(writer)\n    net.Start('a')\n    writer()\n\
             \x20   net.Broadcast()\n  end\nelse\n\
             \x20 net.Receive('a', function() net.ReadString() end)\nend\n",
        );
        expect_no_offenses(
            READER,
            "if SERVER then\n  net.Start('a')\n  net.WriteBool(b)\n  net.Broadcast()\n\
             else\n  net.Receive('a', handler)\nend\n",
        );
    }

    #[test]
    fn helpers_that_do_not_use_the_net_library_are_fine() {
        expect_offense(
            READER,
            &format!(
                "local function clean(s) return s:Trim() end\n{}",
                server_to_client(
                    "  net.WriteString(clean(s))\n",
                    "    local s = clean(net.ReadBool())\n"
                )
            ),
            8,
            21,
            "Value 1 of net message `msg` is written with `net.WriteString` on line 4 but read \
             with `net.ReadBool`.",
        );
    }

    #[test]
    fn replies_and_nested_functions_do_not_count() {
        expect_no_offenses(
            READER,
            &server_to_client(
                "  net.WriteString(s)\n  timer.Simple(0, function() net.WriteBool(b) end)\n",
                "    local s = net.ReadString()\n    net.Start('reply')\n    net.WriteBool(true)\n\
                 \x20   net.SendToServer()\n",
            ),
        );
    }

    #[test]
    fn needs_a_send_and_a_literal_name() {
        expect_no_offenses(
            READER,
            "if SERVER then\n  net.Start('a')\n  net.WriteBool(b)\n  if x then net.Broadcast() end\n\
             \x20 net.Start(name)\n  net.WriteBool(b)\n  net.Broadcast()\n\
             else\n  net.Receive('a', function() net.ReadString() end)\n\
             \x20 net.Receive(name, function() net.ReadString() end)\nend\n",
        );
    }

    /// The number of offenses in `src` inspected as the file `path`.
    fn offenses_in(path: &str, src: &str) -> usize {
        let source = Source::new(path, src);
        let block = parse(&source).expect("parses");
        let file = FileNet::new(&block, &source);
        let sends = file.sends(&block);
        file.receives(&block)
            .iter()
            .map(|receive| check(&source, receive, &sends).len())
            .sum()
    }

    #[test]
    fn realm_files() {
        let src = "if CLIENT then\n  net.Start('a')\n  net.WriteBool(b)\n  net.SendToServer()\n\
                   end\nif SERVER then\n  net.Receive('a', function() net.ReadString() end)\nend\n";
        assert_eq!(offenses_in("sh_test.lua", src), 1);
        assert_eq!(offenses_in("lua/autorun/x.lua", src), 1);
        assert_eq!(
            offenses_in("cl_test.lua", src),
            0,
            "no server code in a client file"
        );
        assert_eq!(offenses_in("lua/autorun/server/x.lua", src), 0);
        let src = "net.Start('a')\nnet.WriteBool(b)\nnet.SendToServer()\n\
                   if SERVER then\n  net.Receive('a', function() net.ReadString() end)\nend\n";
        assert_eq!(
            offenses_in("sh_test.lua", src),
            0,
            "the sender's realm is unknown"
        );
        assert_eq!(offenses_in("cl_test.lua", src), 0);
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(
            READER,
            &format!(
                "{}if then\n",
                server_to_client("  net.WriteBool(b)\n", "    net.ReadString()\n")
            ),
        );
    }
}
