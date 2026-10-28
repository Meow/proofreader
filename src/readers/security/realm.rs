//! Realms and registered handlers: which code of a file may run on the server, and the
//! functions it registers with `net.Receive` or `concommand.Add` there.
//!
//! Code is client-only when the file is (its name starts with `cl_`, as `cl_init.lua` does, or
//! it lies in a `client` directory such as `lua/autorun/client`) or when it is guarded by the
//! realm globals: the body of `if CLIENT then` (also `if CLIENT and x then`, `if not SERVER
//! then`), the `elseif` and `else` branches after `if SERVER then` (or `if not CLIENT then`),
//! and the rest of a block after a guard such as `if SERVER then return end`. Everything else
//! (`sv_`, `sh_`, `init.lua`, `shared.lua`, unguarded code) may run on the server.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::readers::lint::ast::{
    Arguments, Block, Expression, ExpressionKind, Function, Statement, StatementKind, Visitor, walk,
};
use crate::source::Source;
use crate::token::TokenKind;

/// Whether the whole file runs on the client only: its name starts with `cl_` (`cl_init.lua`,
/// `cl_hooks.lua`) or one of its directories is named `client` (`lua/autorun/client`).
pub fn is_client_file(source: &Source) -> bool {
    let path = &source.path;
    let file_client = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("cl_"));
    let directory_client = path
        .parent()
        .into_iter()
        .flat_map(|parent| parent.components())
        .any(|component| component.as_os_str().eq_ignore_ascii_case("client"));
    file_client || directory_client
}

/// The byte ranges of `block` that only run on the client because a realm check guards them.
pub fn client_ranges(block: &Block, source: &Source) -> Vec<Range<usize>> {
    let mut finder = ClientRanges {
        source,
        ranges: Vec::new(),
    };
    walk(block, &mut finder);
    finder.ranges
}

/// Collects the client-only branches and the code after client guards.
struct ClientRanges<'a> {
    source: &'a Source,
    ranges: Vec<Range<usize>>,
}

impl ClientRanges<'_> {
    /// Whether `expression` names the realm global `global` (`CLIENT`, `SERVER`).
    fn is_global(&self, expression: &Expression, global: &str) -> bool {
        matches!(&expression.unparenthesized().kind,
            ExpressionKind::Name(token) if self.source.text_of(token) == global)
    }

    /// Whether `condition` can only be true on the client: `CLIENT`, `not SERVER`, or an
    /// `and` with such an operand.
    fn true_on_client_only(&self, condition: &Expression) -> bool {
        let condition = condition.unparenthesized();
        if self.is_global(condition, "CLIENT") {
            return true;
        }
        match &condition.kind {
            ExpressionKind::Unary { operator, operand }
                if matches!(operator.kind, TokenKind::Not | TokenKind::Bang) =>
            {
                self.false_on_client_only(operand)
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(operator.kind, TokenKind::And | TokenKind::AndAnd) => {
                self.true_on_client_only(left) || self.true_on_client_only(right)
            }
            _ => false,
        }
    }

    /// Whether `condition` can only be false on the client: `SERVER`, `not CLIENT`, or an `or`
    /// with such an operand.
    fn false_on_client_only(&self, condition: &Expression) -> bool {
        let condition = condition.unparenthesized();
        if self.is_global(condition, "SERVER") {
            return true;
        }
        match &condition.kind {
            ExpressionKind::Unary { operator, operand }
                if matches!(operator.kind, TokenKind::Not | TokenKind::Bang) =>
            {
                self.true_on_client_only(operand)
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(operator.kind, TokenKind::Or | TokenKind::OrOr) => {
                self.false_on_client_only(left) || self.false_on_client_only(right)
            }
            _ => false,
        }
    }
}

impl<'t> Visitor<'t> for ClientRanges<'_> {
    fn enter_block(&mut self, block: &'t Block) {
        for statement in &block.statements {
            let StatementKind::If {
                clauses, otherwise, ..
            } = &statement.kind
            else {
                continue;
            };
            let [clause] = clauses.as_slice() else {
                continue;
            };
            let leaves = matches!(
                clause.body.statements.last().map(|last| &last.kind),
                Some(StatementKind::Return { .. })
            );
            if otherwise.is_none() && leaves && self.false_on_client_only(&clause.condition) {
                self.ranges.push(statement.range.end..block.range.end);
                break;
            }
        }
    }

    fn statement(&mut self, statement: &'t Statement) {
        let StatementKind::If {
            clauses, otherwise, ..
        } = &statement.kind
        else {
            return;
        };
        let mut client_after = false;
        for clause in clauses {
            if client_after || self.true_on_client_only(&clause.condition) {
                self.ranges.push(clause.body.range.clone());
            }
            client_after |= self.false_on_client_only(&clause.condition);
        }
        if let (true, Some((keyword, body))) = (client_after, otherwise) {
            self.ranges
                .push(keyword.start..body.range.end.max(keyword.end));
        }
    }
}

/// A function registered as a handler by a call such as `net.Receive('name', handler)`.
#[derive(Debug, Clone, Copy)]
pub struct Handler<'t> {
    /// The registering call.
    pub call: &'t Expression,
    /// The handler.
    pub function: &'t Function,
}

impl Handler<'_> {
    /// The name the handler is registered under, when it is a string literal.
    pub fn name<'a>(&self, source: &'a Source) -> Option<&'a str> {
        let (_, arguments) = registrar_call(self.call, source)?;
        arguments.values.first()?.string_contents(source)
    }
}

/// For a call of a plain function (`a.b(...)`) or a method (`a.b:c(...)`), the name it is
/// registered under in options such as `PayloadReceivers` and its arguments.
fn registrar_call<'t>(
    expression: &'t Expression,
    source: &Source,
) -> Option<(String, &'t Arguments)> {
    match &expression.kind {
        ExpressionKind::MethodCall {
            object,
            name,
            arguments,
        } => Some((
            format!("{}:{}", object.dotted_name(source)?, source.text_of(name)),
            arguments,
        )),
        _ => expression.as_call(source),
    }
}

/// The handlers that calls of `registrar` (`net.Receive`, `concommand.Add`, or a method such as
/// `Clockwork.datastream:Hook`) in `block` register
/// where they may run on the server, in source order, each function once.
///
/// The handler is the second argument: a function literal, or the name of a function defined in
/// the file (`local function f`, `function a.b()`, `local f = function`, `a.b = function`). A
/// handler is left out when the file, the registering call or the handler's definition is
/// client-only.
pub fn server_handlers<'t>(block: &'t Block, source: &Source, registrar: &str) -> Vec<Handler<'t>> {
    if is_client_file(source) {
        return Vec::new();
    }
    let client = client_ranges(block, source);
    let is_client = |offset: usize| client.iter().any(|range| range.contains(&offset));
    let mut finder = HandlerFinder {
        source,
        registrar,
        definitions: HashMap::new(),
        registrations: Vec::new(),
    };
    walk(block, &mut finder);
    let mut seen = HashSet::new();
    let mut handlers = Vec::new();
    for (call, handler) in finder.registrations {
        if is_client(call.range.start) {
            continue;
        }
        let functions: Vec<&Function> = match &handler.unparenthesized().kind {
            ExpressionKind::Function(function) => vec![function],
            _ => handler
                .dotted_name(source)
                .and_then(|name| finder.definitions.get(&name))
                .cloned()
                .unwrap_or_default(),
        };
        for function in functions {
            if !is_client(function.keyword.start) && seen.insert(function.keyword.start) {
                handlers.push(Handler { call, function });
            }
        }
    }
    handlers
}

/// Collects the registering calls and the named function definitions of a file.
struct HandlerFinder<'a, 't> {
    source: &'a Source,
    registrar: &'a str,
    definitions: HashMap<String, Vec<&'t Function>>,
    registrations: Vec<(&'t Expression, &'t Expression)>,
}

impl<'t> HandlerFinder<'_, 't> {
    /// Records that `value`, if it is a function literal, is defined under `name`.
    fn define(&mut self, name: String, value: &'t Expression) {
        if let ExpressionKind::Function(function) = &value.unparenthesized().kind {
            self.definitions.entry(name).or_default().push(function);
        }
    }
}

impl<'t> Visitor<'t> for HandlerFinder<'_, 't> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Function { name, function } => {
                let path: Vec<&str> = name
                    .path
                    .iter()
                    .chain(&name.method)
                    .map(|token| self.source.text_of(token))
                    .collect();
                self.definitions
                    .entry(path.join("."))
                    .or_default()
                    .push(function);
            }
            StatementKind::LocalFunction { name, function, .. } => {
                self.definitions
                    .entry(self.source.text_of(name).to_owned())
                    .or_default()
                    .push(function);
            }
            StatementKind::Local { names, values, .. } => {
                for (name, value) in names.iter().zip(values) {
                    self.define(self.source.text_of(name).to_owned(), value);
                }
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in targets.iter().zip(values) {
                    if let Some(name) = target.dotted_name(self.source) {
                        self.define(name, value);
                    }
                }
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        let Some((function, arguments)) = registrar_call(expression, self.source) else {
            return;
        };
        if function == self.registrar
            && let Some(handler) = arguments.values.get(1)
        {
            self.registrations.push((expression, handler));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::lint::ast::parse;

    /// The registered handlers of `src` inspected as the file `path`, as the line of each
    /// handler's `function` keyword.
    fn handler_lines(path: &str, src: &str, registrar: &str) -> Vec<u32> {
        let source = Source::new(path, src);
        let block = parse(&source).expect("parses");
        server_handlers(&block, &source, registrar)
            .iter()
            .map(|handler| source.position(handler.function.keyword.start).0)
            .collect()
    }

    #[test]
    fn client_files() {
        for path in [
            "cl_init.lua",
            "gamemode/cl_hooks.lua",
            "lua/autorun/client/menu.lua",
            "CL_X.lua",
        ] {
            assert!(is_client_file(&Source::new(path, "")), "{path}");
        }
        for path in [
            "init.lua",
            "shared.lua",
            "sv_hooks.lua",
            "sh_plugin.lua",
            "lua/autorun/server/a.lua",
            "lua/autorun/clientside.lua",
        ] {
            assert!(!is_client_file(&Source::new(path, "")), "{path}");
        }
    }

    #[test]
    fn handlers_of_client_files_are_skipped() {
        let src = "net.Receive('a', function() end)\n";
        assert_eq!(handler_lines("sv_a.lua", src, "net.Receive"), vec![1]);
        assert_eq!(handler_lines("sh_a.lua", src, "net.Receive"), vec![1]);
        assert!(handler_lines("cl_a.lua", src, "net.Receive").is_empty());
    }

    #[test]
    fn realm_branches() {
        let src = "if CLIENT then\n  net.Receive('a', function() end)\nend\n\
                   if SERVER then\n  net.Receive('b', function() end)\n\
                   else\n  net.Receive('c', function() end)\nend\n\
                   if (!SERVER) then net.Receive('d', function() end) end\n\
                   if CLIENT and x then net.Receive('e', function() end) end\n\
                   if SERVER or x then elseif y then net.Receive('f', function() end) end\n\
                   if x then net.Receive('g', function() end) else net.Receive('h', function() end) end\n\
                   if not CLIENT then net.Receive('i', function() end) end\n";
        assert_eq!(
            handler_lines("sh_a.lua", src, "net.Receive"),
            vec![5, 12, 12, 13]
        );
    }

    #[test]
    fn client_guards_cover_the_rest_of_their_block() {
        let src = "net.Receive('a', function() end)\n\
                   if SERVER then AddCSLuaFile() return end\n\
                   net.Receive('b', function() end)\n";
        assert_eq!(handler_lines("sh_a.lua", src, "net.Receive"), vec![1]);
        let src = "if SERVER then x() end\nnet.Receive('b', function() end)\n\
                   if CLIENT then return end\nnet.Receive('c', function() end)\n";
        assert_eq!(handler_lines("sh_a.lua", src, "net.Receive"), vec![2, 4]);
    }

    #[test]
    fn named_handlers() {
        let src = "local function a() end\nfunction PLUGIN.b() end\nlocal c = function() end\n\
                   if CLIENT then function d() end end\n\
                   net.Receive('a', a)\nnet.Receive('b', PLUGIN.b)\nnet.Receive('c', c)\n\
                   net.Receive('d', d)\nnet.Receive('e', unknown)\nnet.Receive('a2', a)\n";
        assert_eq!(handler_lines("sv_a.lua", src, "net.Receive"), vec![1, 2, 3]);
    }

    #[test]
    fn registered_names() {
        let source = Source::new("sv_a.lua", "concommand.Add('x', function() end)\n");
        let block = parse(&source).expect("parses");
        let handlers = server_handlers(&block, &source, "concommand.Add");
        assert_eq!(handlers[0].name(&source), Some("x"));
    }
}
