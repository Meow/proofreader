//! `Security/NetReadTable`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{Expression, Visitor, parse, walk};
use crate::readers::security::realm::server_handlers;
use crate::source::Source;

/// Flags `net.ReadTable()` inside a `net.Receive` handler that may run on the server: a client
/// can send an arbitrarily large or deeply nested table (or values of any type in it) and stall
/// or confuse the server. The handler is a function literal passed to `net.Receive` or a
/// function of the file passed by name; functions nested in it count. Handlers registered in
/// client-only code (a `cl_` file, a `client` directory, an `if CLIENT then` branch, the `else`
/// of `if SERVER then`) are exempt. Files that do not parse are skipped.
pub struct NetReadTable;

impl Reader for NetReadTable {
    fn name(&self) -> &'static str {
        "Security/NetReadTable"
    }

    fn description(&self) -> &'static str {
        "Checks for `net.ReadTable()` in net message handlers that run on the server."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let Ok(block) = parse(source) else {
            return;
        };
        let mut finder = Finder {
            source,
            found: Vec::new(),
        };
        for handler in server_handlers(&block, source, "net.Receive") {
            walk(&handler.function.body, &mut finder);
        }
        finder.found.sort_by_key(|range| range.start);
        for range in finder.found {
            ctx.add_offense(
                range,
                "`net.ReadTable` lets a client send any table; read typed values (`net.ReadUInt`, `net.ReadString`, ...) instead.",
            );
        }
    }
}

/// Collects the callees of `net.ReadTable()` calls.
struct Finder<'a> {
    source: &'a Source,
    found: Vec<std::ops::Range<usize>>,
}

impl<'t> Visitor<'t> for Finder<'_> {
    fn expression(&mut self, expression: &'t Expression) {
        if let Some((function, _)) = expression.as_call(self.source)
            && function == "net.ReadTable"
        {
            self.found.push(expression.range.clone());
        }
    }
}

inventory::submit! { Registration(|| Box::new(NetReadTable)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Security/NetReadTable";

    const MESSAGE: &str = "`net.ReadTable` lets a client send any table; read typed values (`net.ReadUInt`, `net.ReadString`, ...) instead.";

    #[test]
    fn flags_read_table_in_server_handlers() {
        expect_offenses(
            READER,
            "net.Receive('a', function(len, ply)\n  local data = net.ReadTable()\n\
             timer.Simple(0, function() f(net.ReadTable()) end)\nend)\n",
            &[(2, 16, MESSAGE), (3, 30, MESSAGE)],
        );
    }

    #[test]
    fn flags_short_function_handlers() {
        expect_offenses(
            READER,
            "net.Receive('a', |len, ply| -> handle(net.ReadTable()))\n\
             net.Receive('b', || -> do\n  local data = net.ReadTable()\nend)\n",
            &[(1, 39, MESSAGE), (3, 16, MESSAGE)],
        );
    }

    #[test]
    fn flags_named_handlers() {
        expect_offense(
            READER,
            "local function receive(len, ply)\n  local data = net.ReadTable()\nend\n\
             net.Receive('a', receive)\n",
            2,
            16,
            MESSAGE,
        );
    }

    #[test]
    fn accepts_client_handlers() {
        expect_no_offenses(
            READER,
            "if CLIENT then\n  net.Receive('a', function() local t = net.ReadTable() end)\nend\n\
             if SERVER then\n  net.Receive('b', function() local s = net.ReadString() end)\n\
             else\n  net.Receive('c', function() local t = net.ReadTable() end)\nend\n",
        );
    }

    #[test]
    fn accepts_read_table_outside_handlers() {
        expect_no_offenses(
            READER,
            "local function read() return net.ReadTable() end\nlocal t = net.ReadTable()\n\
             net.Receive('a', function() read() end)\n",
        );
    }

    #[test]
    fn skips_unparsable_files() {
        expect_no_offenses(
            READER,
            "net.Receive('a', function() local t = net.ReadTable() end)\nif then\n",
        );
    }
}
