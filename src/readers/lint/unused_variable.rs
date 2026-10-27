//! `Lint/UnusedVariable`.

use std::collections::HashMap;
use std::ops::Range;

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::ast::{
    self, Block, Expression, ExpressionKind, FieldKey, Statement, StatementKind, Visitor,
};
use crate::readers::lint::scopes::{Resolution, Variable, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::readers::style::doc_comments::{LDOC_TYPED_PARAM_TAGS, doc_comment};
use crate::source::Source;
use crate::token::Token;

/// Flags local variables, `local function`s, parameters and loop variables that are never read.
///
/// Component tags of Lumen templates (`<Command />`) read the variable they name. A variable
/// that is assigned after its declaration but never read gets its own message. Names starting
/// with `_` (including `_` itself), `self` and `...` are exempt. Files that do not parse are
/// skipped.
///
/// The fix removes an unused local together with every assignment to it: a statement declaring
/// or assigning only that name is deleted (with its line when nothing else is on it, and with
/// the doc comment of a removed function), and one declaring several names loses the name and
/// its value. A value that may have side effects stays: `local x = f()` becomes `f()`, and other
/// such values (`local x = f() + 1`) are not corrected. Parameters and loop variables cannot be
/// removed, so their fix prefixes the name with `_` at the declaration, at every assignment and,
/// for a parameter, in the matching `@param` tags of the doc comment right above its function;
/// there is no fix when the prefixed name already appears in the variable's scope. With
/// `AllowUnusedParameters`, parameters are never flagged (GMod hooks and callbacks receive
/// arguments they often do not need).
pub struct UnusedVariable;

impl Reader for UnusedVariable {
    fn name(&self) -> &'static str {
        "Lint/UnusedVariable"
    }

    fn description(&self) -> &'static str {
        "Checks for variables that are declared but never read."
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![("AllowUnusedParameters", Yaml::Boolean(false))]
    }

    fn investigate(&self, ctx: &mut Context) {
        let allow_parameters = ctx.option_bool("AllowUnusedParameters", false);
        let source = ctx.source;
        let Some(resolution) = resolve(source) else {
            return;
        };
        let tree = ast::parse(source).ok();
        let bindings = tree.as_ref().map(bindings).unwrap_or_default();
        for variable in &resolution.variables {
            if variable.implicit
                || variable.reads > 0
                || variable.name.starts_with('_')
                || variable.name == "self"
                || (allow_parameters && variable.kind == DeclarationKind::Parameter)
            {
                continue;
            }
            let range = variable.token.range();
            let message = message(variable);
            let edits = match variable.kind {
                DeclarationKind::Local | DeclarationKind::LocalFunction => {
                    removal(source, &bindings, variable)
                }
                DeclarationKind::Parameter | DeclarationKind::ForVariable => {
                    prefixing(source, &resolution, variable)
                }
            };
            match edits {
                Some(edits) => ctx.add_offense_with_fix(range, message, edits),
                None => ctx.add_offense(range, message),
            };
        }
    }
}

/// The offense message for the unread `variable`.
fn message(variable: &Variable) -> String {
    let (noun, capitalized, remedy) = match variable.kind {
        DeclarationKind::Local => ("local variable", "Local variable", "Remove it."),
        DeclarationKind::LocalFunction => ("local function", "Local function", "Remove it."),
        DeclarationKind::Parameter => ("parameter", "Parameter", PREFIX_REMEDY),
        DeclarationKind::ForVariable => ("loop variable", "Loop variable", PREFIX_REMEDY),
    };
    let name = &variable.name;
    if variable.writes.is_empty() {
        format!("Unused {noun} `{name}`. {remedy}")
    } else {
        format!("{capitalized} `{name}` is assigned but never used. {remedy}")
    }
}

/// What the messages for parameters and loop variables tell the user to do.
const PREFIX_REMEDY: &str = "Remove it or prefix it with `_`.";

/// A statement binding a name, keyed in [`bindings`] by the offset of that name's token.
#[derive(Clone, Copy)]
struct Binding<'t> {
    /// The `local`, `local function`, single-target assignment or `function name()` statement.
    statement: &'t Statement,
    /// The position of the name among the statement's names.
    index: usize,
}

/// Collects every statement that declares or assigns a plain name, by the name's offset.
#[derive(Default)]
struct Bindings<'t>(HashMap<usize, Binding<'t>>);

impl<'t> Visitor<'t> for Bindings<'t> {
    fn statement(&mut self, statement: &'t Statement) {
        let mut add = |token: &Token, index| {
            self.0.insert(token.start, Binding { statement, index });
        };
        match &statement.kind {
            StatementKind::Local { names, .. } => {
                for (index, name) in names.iter().enumerate() {
                    add(name, index);
                }
            }
            StatementKind::LocalFunction { name, .. } => add(name, 0),
            StatementKind::Assignment { targets, .. } => {
                if let [target] = targets.as_slice()
                    && let ExpressionKind::Name(name) = &target.kind
                {
                    add(name, 0);
                }
            }
            StatementKind::Function { name, .. } => {
                if let ([path], None) = (name.path.as_slice(), name.method) {
                    add(path, 0);
                }
            }
            _ => {}
        }
    }
}

/// The name-binding statements of `tree`.
fn bindings(tree: &Block) -> Bindings<'_> {
    let mut bindings = Bindings::default();
    ast::walk(tree, &mut bindings);
    bindings
}

/// The edits removing the local `variable`: its declaration and every assignment to it. `None`
/// when one of them cannot be removed without changing what the code does.
fn removal(source: &Source, bindings: &Bindings, variable: &Variable) -> Option<Vec<Edit>> {
    let mut edits = Vec::new();
    for token in std::iter::once(&variable.token).chain(&variable.writes) {
        edits.extend(unbind(source, *bindings.0.get(&token.start)?)?);
    }
    Some(edits)
}

/// The edits removing the name of `binding` from its statement.
fn unbind(source: &Source, binding: Binding) -> Option<Vec<Edit>> {
    let statement = binding.statement;
    let (names, values): (Vec<Range<usize>>, &[Expression]) = match &statement.kind {
        StatementKind::Local { names, values, .. } => {
            (names.iter().map(Token::range).collect(), values)
        }
        StatementKind::Assignment {
            targets, values, ..
        } => (
            targets.iter().map(|target| target.range.clone()).collect(),
            values,
        ),
        StatementKind::LocalFunction { function, .. }
        | StatementKind::Function { function, .. } => {
            return Some(vec![remove_statement(
                source,
                statement,
                Some(&function.keyword),
            )]);
        }
        _ => return None,
    };
    if names.len() == 1 {
        if let [value] = values
            && let ExpressionKind::Function(function) = &value.kind
        {
            return Some(vec![remove_statement(
                source,
                statement,
                Some(&function.keyword),
            )]);
        }
        if values.iter().all(droppable) {
            return Some(vec![remove_statement(source, statement, None)]);
        }
        // Keep a call for its side effects; a statement starting with `(` could continue the
        // previous one.
        if let [value] = values
            && value.is_call()
            && !source.text[value.range.clone()].starts_with('(')
        {
            return Some(vec![Edit::remove(statement.range.start..value.range.start)]);
        }
        return None;
    }
    let index = binding.index;
    let mut edits = vec![Edit::remove(list_item(&names, index))];
    if let Some(value) = values.get(index) {
        // A multi-valued last value also fills the names after it.
        let expands = index + 1 == values.len() && values.len() < names.len();
        if !droppable(value) || (expands && value.is_multi_valued()) {
            return None;
        }
        let ranges: Vec<Range<usize>> = values.iter().map(|value| value.range.clone()).collect();
        edits.push(Edit::remove(list_item(&ranges, index)));
    }
    Some(edits)
}

/// The range removing item `index` of a comma-separated list with the comma after it (or, for
/// the last item, the comma before it).
fn list_item(items: &[Range<usize>], index: usize) -> Range<usize> {
    match items.get(index + 1) {
        Some(next) => items[index].start..next.start,
        None => items[index - 1].end..items[index].end,
    }
}

/// Whether evaluating `expression` has no effect besides producing its value: anything but a
/// call (and markup or tokens the parser does not know).
fn droppable(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Nil
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::Number
        | ExpressionKind::String(_)
        | ExpressionKind::Vararg
        | ExpressionKind::Function(_)
        | ExpressionKind::Name(_) => true,
        ExpressionKind::Table(table) => table.fields.iter().all(|field| {
            droppable(&field.value)
                && match &field.key {
                    FieldKey::Bracketed(key) => droppable(key),
                    FieldKey::Positional | FieldKey::Named(_) => true,
                }
        }),
        ExpressionKind::Parenthesized(inner) => droppable(inner),
        ExpressionKind::Field { object, .. } => droppable(object),
        ExpressionKind::Index { object, key } => droppable(object) && droppable(key),
        ExpressionKind::Unary { operand, .. } => droppable(operand),
        ExpressionKind::Binary { left, right, .. } => droppable(left) && droppable(right),
        ExpressionKind::Call { .. }
        | ExpressionKind::MethodCall { .. }
        | ExpressionKind::Markup(_)
        | ExpressionKind::Opaque => false,
    }
}

/// The edit removing `statement` and a `;` right after it. A statement alone on its lines (but
/// for a trailing comment) is removed with those lines and, for a function whose `function`
/// keyword is `function`, with the doc comment above it (comment-only lines including a `---` one); otherwise the statement goes
/// with the blanks after it.
fn remove_statement(source: &Source, statement: &Statement, function: Option<&Token>) -> Edit {
    let text = &source.text;
    let mut start = statement.range.start;
    let mut end = statement.range.end;
    let blanks = |from: usize| {
        text[from..]
            .find(|c: char| c != ' ' && c != '\t')
            .map_or(text.len(), |length| from + length)
    };
    if text[blanks(end)..].starts_with(';') {
        end = blanks(end) + 1;
    }
    let (first_line, _) = source.position(start);
    let (last_line, _) = source.position(end);
    let line_start = source.line_range(first_line).start;
    let line_end = source.line_range(last_line).end;
    // A comment after the statement on its last line belongs to it.
    let only_comments_after = source
        .code_tokens_on_line(last_line)
        .filter(|token| token.start >= end)
        .all(|token| token.kind.is_comment() && token.end <= line_end);
    let alone = text[line_start..start].trim().is_empty() && only_comments_after;
    if !alone {
        let after = blanks(end);
        if after == line_end {
            start = text[..start].trim_end_matches([' ', '\t']).len();
        } else {
            end = after;
        }
        return Edit::remove(start..end);
    }
    let first_line = function
        .and_then(|keyword| doc_comment(source, keyword))
        .filter(|doc| {
            doc.comments
                .iter()
                .any(|comment| source.text_of(comment).starts_with("---"))
        })
        .map_or(first_line, |doc| doc.comments[0].line);
    start = source.line_range(first_line).start;
    if line_end < text.len() {
        end = line_end + 1;
    } else {
        // The last line has no line break of its own: take the one before it.
        end = line_end;
        start = start.saturating_sub(1);
    }
    Edit::remove(start..end)
}

/// The edits prefixing `variable` with `_`, or `None` when the prefixed name already appears in
/// its scope.
fn prefixing(source: &Source, resolution: &Resolution, variable: &Variable) -> Option<Vec<Edit>> {
    let renamed = format!("_{}", variable.name);
    if resolution.mentions(&renamed, &variable.scope) {
        return None;
    }
    let mut edits: Vec<Edit> = std::iter::once(&variable.token)
        .chain(&variable.writes)
        .map(|token| Edit::insert(token.start, "_"))
        .collect();
    if let Some(keyword) = variable.function {
        edits.extend(
            param_tags(source, &keyword, &variable.name)
                .into_iter()
                .map(|offset| Edit::insert(offset, "_")),
        );
    }
    Some(edits)
}

/// Byte offsets of the parameter names in the parameter tags for `name` (`@param name`,
/// `@tparam type name`, `@string name`, ...) of the doc comment of the function whose keyword is
/// `keyword` (see [`doc_comment`]).
fn param_tags(source: &Source, keyword: &Token, name: &str) -> Vec<usize> {
    doc_comment(source, keyword).map_or_else(Vec::new, |doc| {
        doc.params(&LDOC_TYPED_PARAM_TAGS)
            .into_iter()
            .filter(|param| param.name == name)
            .map(|param| param.range.start)
            .collect()
    })
}

inventory::submit! { Registration(|| Box::new(UnusedVariable)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/UnusedVariable";

    /// What the message for a variable of `kind` tells the user to do.
    fn remedy(kind: &str) -> &'static str {
        if kind.to_lowercase().starts_with("local") {
            "Remove it."
        } else {
            "Remove it or prefix it with `_`."
        }
    }

    fn unused(kind: &str, name: &str) -> String {
        format!("Unused {kind} `{name}`. {}", remedy(kind))
    }

    fn assigned(kind: &str, name: &str) -> String {
        format!(
            "{kind} `{name}` is assigned but never used. {}",
            remedy(kind)
        )
    }

    #[test]
    fn parameters_can_be_allowed() {
        let src = "function M.f(a, b) local c return b end\n";
        let offenses = inspect_with(READER, src, "AllowUnusedParameters: true");
        assert_eq!(offenses.len(), 1);
        assert_eq!(offenses[0].message, unused("local variable", "c"));
        assert_eq!(inspect(READER, src).len(), 2);
    }

    #[test]
    fn flags_every_declaration_kind() {
        expect_offenses(
            READER,
            "local a, b = 1, 2\nprint(b)\n",
            &[(1, 7, &unused("local variable", "a"))],
        );
        expect_offense(
            READER,
            "local function helper() end\n",
            1,
            16,
            &unused("local function", "helper"),
        );
        expect_offense(
            READER,
            "function M.f(a, b) return b end\n",
            1,
            14,
            &unused("parameter", "a"),
        );
        expect_offense(
            READER,
            "function M:f(a) end\n",
            1,
            14,
            &unused("parameter", "a"),
        );
        expect_offense(
            READER,
            "hook.Add('Think', 'x', function(actor) end)\n",
            1,
            33,
            &unused("parameter", "actor"),
        );
        expect_offenses(
            READER,
            "for k, v in pairs(t) do print(v) end\nfor i = 1, 2 do end\n",
            &[
                (1, 5, &unused("loop variable", "k")),
                (2, 5, &unused("loop variable", "i")),
            ],
        );
    }

    #[test]
    fn lumen_component_tags_read_their_variable() {
        expect_no_offenses_lumen(
            READER,
            "local function Row(props)\n  return <text>{props.label}</text>\nend\n\n\
             return function(props)\n  return <view><Row label={props.label} /></view>\nend\n",
        );
        expect_offenses_lumen(
            READER,
            "local function Row(props)\n  return <text>row</text>\nend\n\n\
             return function()\n  return <view><row /></view>\nend\n",
            &[
                (1, 16, &unused("local function", "Row")),
                (1, 20, &unused("parameter", "props")),
            ],
        );
        expect_offense_lumen(
            READER,
            "local Card = 1\nlocal function f()\n  local Card = 2\n  return <Card />\nend\n\n\
             return f\n",
            1,
            7,
            &unused("local variable", "Card"),
        );
    }

    #[test]
    fn lumen_splices_are_parsed() {
        expect_offenses_lumen(
            READER,
            "for k, v in ipairs(list) do print(v) end\n\nreturn function(props)\n  \
             return <view style={{ gap = props.gap }} {...props.rest}>\n    \
             {props.title}\n    {}\n    {props.open and <text>{props.body}</text>}\n    \
             {Lumen.map(props.items, function(item, index)\n      \
             return <text key={item.id}>{item.name}</text>\n    end)}\n  </view>\nend\n",
            &[
                (1, 5, &unused("loop variable", "k")),
                (8, 44, &unused("parameter", "index")),
            ],
        );
    }

    #[test]
    fn lumen_text_may_start_a_line_with_a_brace() {
        expect_offense_lumen(
            READER,
            "return function(props, extra)\n  return <view>{f(<text>\n    } x\n  </text>, props)}\
             </view>\nend\n",
            1,
            24,
            &unused("parameter", "extra"),
        );
    }

    #[test]
    fn accepts_used_variables() {
        expect_no_offenses(
            READER,
            "local a = 1\nlocal function f(x) return x + a end\nprint(f(2))\n\
             for k, v in pairs(t) do t2[k] = v end\nfor i = 1, 2 do print(i) end\n",
        );
    }

    #[test]
    fn exempts_underscores_self_and_varargs() {
        expect_no_offenses(
            READER,
            "local _ = 1\nlocal _unused = 2\nfunction f(_a, _, ...) end\n\
             for _, v in pairs(t) do print(v) end\nlocal self = 1\nfunction M:g() end\n",
        );
    }

    #[test]
    fn shadowing_hides_the_outer_variable() {
        expect_offense(
            READER,
            "local a = 1\nlocal a = 2\nprint(a)\n",
            1,
            7,
            &unused("local variable", "a"),
        );
        expect_offense(
            READER,
            "local a = 1\ndo local a = 2 print(a) end\n",
            1,
            7,
            &unused("local variable", "a"),
        );
        expect_offense(
            READER,
            "function f(a) local a = 1 return a end\n",
            1,
            12,
            &unused("parameter", "a"),
        );
    }

    #[test]
    fn local_right_hand_side_reads_the_outer_variable() {
        expect_offense(
            READER,
            "local x = 1\nlocal x = x\n",
            2,
            7,
            &unused("local variable", "x"),
        );
        expect_offense(
            READER,
            "local x = x\n",
            1,
            7,
            &unused("local variable", "x"),
        );
    }

    #[test]
    fn recursion_counts_for_local_functions_only() {
        expect_no_offenses(READER, "local function f(n) return f(n - 1) end\n");
        expect_offense(
            READER,
            "local f = function(n) return f(n - 1) end\n",
            1,
            7,
            &unused("local variable", "f"),
        );
    }

    #[test]
    fn repeat_condition_sees_the_body() {
        expect_no_offenses(READER, "repeat local done = step() until done\n");
        expect_offense(
            READER,
            "repeat local done = step() until x\n",
            1,
            14,
            &unused("local variable", "done"),
        );
    }

    #[test]
    fn closures_read_outer_variables() {
        expect_no_offenses(
            READER,
            "local count = 0\ntimer.Simple(1, function()\n  return function() return count end\nend)\n",
        );
    }

    #[test]
    fn if_branches_are_separate_scopes() {
        expect_offense(
            READER,
            "if x then local a = 1 else print(a) end\n",
            1,
            17,
            &unused("local variable", "a"),
        );
    }

    #[test]
    fn fields_keys_and_labels_are_not_reads() {
        expect_offenses(
            READER,
            "local x, y, z = 1, 2, 3\nprint(t.x, t:y(), { z = 1 })\n",
            &[
                (1, 7, &unused("local variable", "x")),
                (1, 10, &unused("local variable", "y")),
                (1, 13, &unused("local variable", "z")),
            ],
        );
        expect_no_offenses(READER, "local k = 1\nprint({ [k] = 1 })\n");
        expect_offense(
            READER,
            "local skip = 1\ngoto skip\n::skip::\n",
            1,
            7,
            &unused("local variable", "skip"),
        );
    }

    #[test]
    fn indexed_and_method_targets_read_the_base() {
        expect_no_offenses(
            READER,
            "local t = {}\nt.x = 1\nlocal u = {}\nu[1] = 2\nlocal m = {}\nfunction m.f() end\n\
             local o = {}\nfunction o:g() end\n",
        );
    }

    #[test]
    fn write_only_variables_get_their_own_message() {
        expect_offense(
            READER,
            "local x\nx = 1\n",
            1,
            7,
            &assigned("Local variable", "x"),
        );
        expect_offense(
            READER,
            "local f\nfunction f() end\n",
            1,
            7,
            &assigned("Local variable", "f"),
        );
        expect_offense(
            READER,
            "function g(a) a = 1 end\n",
            1,
            12,
            &assigned("Parameter", "a"),
        );
        expect_no_offenses(READER, "local x = 0\nx = x + 1\n");
    }

    #[test]
    fn multiple_assignment() {
        expect_offenses(
            READER,
            "local a, b, c\na, b, c = 1, 2, 3\nprint(b)\n",
            &[
                (1, 7, &assigned("Local variable", "a")),
                (1, 13, &assigned("Local variable", "c")),
            ],
        );
    }

    #[test]
    fn skips_files_that_do_not_parse() {
        expect_no_offenses(READER, "local a = 1\nif a then\n");
    }

    #[test]
    fn removes_unused_locals_with_their_lines() {
        expect_correction(READER, "local A = A\nprint(1)\n", "print(1)\n");
        expect_correction(
            READER,
            "local x = 1\nlocal y = { a = 1, [k] = t.v, -n }\nlocal z\nreturn 1\n",
            "return 1\n",
        );
        expect_correction(
            READER,
            "if a then\n  local x = a .. 'b';\n  print(a)\nend\n",
            "if a then\n  print(a)\nend\n",
        );
        expect_correction(READER, "print(1)\nlocal x = 1", "print(1)");
        expect_correction(READER, "local x = 1", "");
        expect_correction(
            READER,
            "local x = 0 -- luacheck: ignore\nlocal y = 1 --[[a]] -- b\nreturn 1\n",
            "return 1\n",
        );
    }

    #[test]
    fn removes_unused_locals_sharing_a_line() {
        expect_correction(READER, "do local x = 1 end\n", "do end\n");
        expect_correction(READER, "local x = 1 print(2)\n", "print(2)\n");
        expect_correction(READER, "print(2) local x = 1\n", "print(2)\n");
        expect_correction(READER, "print(2) local x = 1 -- c\n", "print(2) -- c\n");
    }

    #[test]
    fn removes_unused_functions_with_their_doc_comment() {
        expect_correction(
            READER,
            "-- Helpers.\n\n--- Does things.\n-- @param a [Number]\nlocal function helper(a)\n  \
             return a\nend\n\nreturn 1\n",
            "-- Helpers.\n\n\nreturn 1\n",
        );
        expect_correction(
            READER,
            "-- Not a doc comment.\nlocal f = function()\nend\nreturn 1\n",
            "-- Not a doc comment.\nreturn 1\n",
        );
    }

    #[test]
    fn removes_the_assignments_of_write_only_locals() {
        expect_correction(
            READER,
            "local x\nx = 1\nif y then x = 2 end\nreturn y\n",
            "if y then end\nreturn y\n",
        );
        expect_correction(
            READER,
            "local f\nfunction f() end\nreturn 1\n",
            "return 1\n",
        );
        expect_correction(
            READER,
            "local done\nhook.Add('Think', 'x', function()\n  done = true\nend)\n",
            "hook.Add('Think', 'x', function()\nend)\n",
        );
        expect_correction(READER, "local x\ndo local x\nx = 1 end\n", "do\nend\n");
    }

    #[test]
    fn keeps_calls_for_their_side_effects() {
        expect_correction(
            READER,
            "local x = f(1)\nlocal y = obj:m 'a'\nlocal z\nz = g()\n",
            "f(1)\nobj:m 'a'\ng()\n",
        );
        let src = "local x = f() + 1\nlocal y = (f or g)()\nlocal z = { f() }\n";
        assert_eq!(autocorrect(READER, src), src);
        assert!(
            inspect(READER, src)
                .iter()
                .all(|offense| !offense.correctable())
        );
        let src = "local x\nx, y = 1, 2\nprint(y)\n";
        assert_eq!(autocorrect(READER, src), src);
    }

    #[test]
    fn removes_one_name_of_several() {
        expect_correction(
            READER,
            "local a, b = 1, 2\nprint(b)\n",
            "local b = 2\nprint(b)\n",
        );
        expect_correction(
            READER,
            "local a, b, c = 1, 2, f()\nprint(a, c)\n",
            "local a, c = 1, f()\nprint(a, c)\n",
        );
        expect_correction(
            READER,
            "local a, b = f()\nprint(a)\nlocal c, d\nprint(c)\n",
            "local a = f()\nprint(a)\nlocal c\nprint(c)\n",
        );
        expect_correction(READER, "local a, b = 1, 2\n", "");
        let src = "local a, b = f()\nprint(b)\nlocal c, d, e = 1, ...\nprint(c, e)\n";
        assert_eq!(autocorrect(READER, src), src);
    }

    #[test]
    fn prefixes_parameters_and_loop_variables() {
        expect_correction(
            READER,
            "for k, v in pairs(t) do print(v) end\nfunction M.f(a) a = 1 end\n",
            "for _k, v in pairs(t) do print(v) end\nfunction M.f(_a) _a = 1 end\n",
        );
    }

    #[test]
    fn renames_the_param_tag_of_the_doc_comment() {
        expect_correction(
            READER,
            "--- Does things.\n-- @param actor [Player]\n-- @param amount=1 [Number]\n\
             function M:give(actor, amount)\n  return actor\nend\n",
            "--- Does things.\n-- @param actor [Player]\n-- @param _amount=1 [Number]\n\
             function M:give(actor, _amount)\n  return actor\nend\n",
        );
        expect_correction(
            READER,
            "--- Does things.\n-- @param a_b [Number]\n-- @param a [Number]\nfunction f(a, a_b) end\n",
            "--- Does things.\n-- @param _a_b [Number]\n-- @param _a [Number]\nfunction f(_a, _a_b) end\n",
        );
    }

    #[test]
    fn leaves_comments_of_other_functions_alone() {
        expect_correction(
            READER,
            "-- @param a [Number]\nlocal x = 1 print(x)\nfunction f(a) end\n",
            "-- @param a [Number]\nlocal x = 1 print(x)\nfunction f(_a) end\n",
        );
        expect_correction(
            READER,
            "-- @param a [Number]\nf(function() end, function(a) end)\n",
            "-- @param a [Number]\nf(function() end, function(_a) end)\n",
        );
    }

    #[test]
    fn keeps_the_offense_when_the_prefixed_name_is_taken() {
        let src = "local _x = 1\nfunction f(x) return _x end\n";
        expect_offense(READER, src, 2, 12, &unused("parameter", "x"));
        assert_eq!(autocorrect(READER, src), src);
        assert!(!inspect(READER, src)[0].correctable());
        let src = "function f(x) x = 1 return _x end\n";
        assert_eq!(autocorrect(READER, src), src);
        expect_correction(
            READER,
            "do local function f(x) end f() end\nprint(_x)\n",
            "do local function f(_x) end f() end\nprint(_x)\n",
        );
    }

    #[test]
    fn varargs_are_not_variables() {
        expect_no_offenses(
            READER,
            "function f(...) end\nlocal g = function(...) end\nprint(g)\n",
        );
        expect_offense(
            READER,
            "function f(a, ...) return ... end\n",
            1,
            12,
            &unused("parameter", "a"),
        );
    }

    #[test]
    fn locals_read_only_inside_their_block_or_after_it() {
        expect_no_offenses(
            READER,
            "local a = 1\ndo print(a) end\nlocal b\nwhile b do b = step() end\n\
             for i = 1, 10, 2 do print(i) end\nlocal t = { 1; 2 }\nprint(t)\n",
        );
        expect_offense(
            READER,
            "do local a = 1 end\nprint(a)\n",
            1,
            10,
            &unused("local variable", "a"),
        );
    }

    #[test]
    fn renames_indented_variant_param_tags() {
        expect_correction(
            READER,
            "--- Does things.\n--   @param a [Number first variant]\n--   @param a [String second variant]\n\
             function f(a) end\n",
            "--- Does things.\n--   @param _a [Number first variant]\n--   @param _a [String second variant]\n\
             function f(_a) end\n",
        );
    }

    #[test]
    fn handles_a_byte_order_mark() {
        expect_offense(
            READER,
            "\u{feff}local a = 1\n",
            1,
            10,
            &unused("local variable", "a"),
        );
    }
}
