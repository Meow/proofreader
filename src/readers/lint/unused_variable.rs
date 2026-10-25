//! `Lint/UnusedVariable`.

use yaml_rust2::Yaml;

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::scopes::{Resolution, Variable, resolve};
use crate::readers::naming::declarations::DeclarationKind;
use crate::readers::style::doc_comments::{LDOC_TYPED_PARAM_TAGS, doc_comment};
use crate::source::Source;
use crate::token::Token;

/// What every message tells the user to do.
const REMEDY: &str = "Remove it or prefix it with `_`.";

/// Flags local variables, `local function`s, parameters and loop variables that are never read.
///
/// Component tags of Lumen templates (`<Command />`) read the variable they name. A variable
/// that is assigned after its declaration but never read gets its own message. Names starting
/// with `_` (including `_` itself), `self` and `...` are exempt. Files that do not parse are
/// skipped. The fix prefixes the name with `_` at the declaration, at every assignment and, for
/// a parameter, in the matching `@param` tags of the doc comment right above its function; there
/// is no fix when the prefixed name already appears in the variable's scope. With
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
            match fix(source, &resolution, variable) {
                Some(edits) => ctx.add_offense_with_fix(range, message, edits),
                None => ctx.add_offense(range, message),
            };
        }
    }
}

/// The offense message for the unread `variable`.
fn message(variable: &Variable) -> String {
    let (noun, capitalized) = match variable.kind {
        DeclarationKind::Local => ("local variable", "Local variable"),
        DeclarationKind::LocalFunction => ("local function", "Local function"),
        DeclarationKind::Parameter => ("parameter", "Parameter"),
        DeclarationKind::ForVariable => ("loop variable", "Loop variable"),
    };
    let name = &variable.name;
    if variable.writes.is_empty() {
        format!("Unused {noun} `{name}`. {REMEDY}")
    } else {
        format!("{capitalized} `{name}` is assigned but never used. {REMEDY}")
    }
}

/// The edits prefixing `variable` with `_`, or `None` when the prefixed name already appears in
/// its scope.
fn fix(source: &Source, resolution: &Resolution, variable: &Variable) -> Option<Vec<Edit>> {
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

    fn unused(kind: &str, name: &str) -> String {
        format!("Unused {kind} `{name}`. Remove it or prefix it with `_`.")
    }

    fn assigned(kind: &str, name: &str) -> String {
        format!("{kind} `{name}` is assigned but never used. Remove it or prefix it with `_`.")
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
    fn prefixes_the_declaration_and_writes() {
        expect_correction(
            READER,
            "local a, b = 1, 2\nprint(b)\nlocal x\nx = 1\nif y then x = 2 end\n",
            "local _a, b = 1, 2\nprint(b)\nlocal _x\n_x = 1\nif y then _x = 2 end\n",
        );
        expect_correction(
            READER,
            "for k, v in pairs(t) do print(v) end\nlocal function f() end\n",
            "for _k, v in pairs(t) do print(v) end\nlocal function _f() end\n",
        );
        expect_correction(
            READER,
            "local f\nfunction f() end\n",
            "local _f\nfunction _f() end\n",
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
        let src = "local _x = 1\nlocal x = 2\nprint(_x)\n";
        expect_offense(READER, src, 2, 7, &unused("local variable", "x"));
        assert_eq!(autocorrect(READER, src), src);
        let offenses = inspect(READER, src);
        assert!(!offenses[0].correctable());
        let src = "local _x, x = 1, 2\nprint(_x)\n";
        assert_eq!(autocorrect(READER, src), src);
        let src = "function f(x) x = 1 return _x end\n";
        assert_eq!(autocorrect(READER, src), src);
        expect_correction(
            READER,
            "do local x = 1 end\nprint(_x)\n",
            "do local _x = 1 end\nprint(_x)\n",
        );
    }

    #[test]
    fn correction_of_shadowed_names_is_consistent() {
        expect_correction(
            READER,
            "local x\ndo local x\nx = 1 end\n",
            "local _x\ndo local _x\n_x = 1 end\n",
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
    fn writes_inside_closures_are_renamed() {
        expect_correction(
            READER,
            "local done\nhook.Add('Think', 'x', function()\n  done = true\nend)\n",
            "local _done\nhook.Add('Think', 'x', function()\n  _done = true\nend)\n",
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
