//! Hot paths: the functions GMod calls every frame or tick and the calls made inside them, plus
//! small expression helpers shared by the performance readers.
//!
//! A function is a hot path when it is defined under a hot path name (`function GM:Think()`,
//! `function PANEL:Paint(w, h)`, `ENT.Draw = function(self)`, `{ Think = function() end }`) or
//! registered for a hot path hook (`hook.Add('HUDPaint', 'id', function() end)`, where the
//! handler may also name a function defined elsewhere in the file). Functions defined inside a
//! hot path are part of it.
//!
//! The hot path names and the hook registration functions are the `HotPaths` and
//! `HookFunctions` options of the `Performance` department.

use std::collections::HashMap;
use std::ops::Range;

use crate::reader::{Context, SharedOptions, yaml_strings};
use crate::readers::lint::ast::{
    Arguments, Block, Expression, ExpressionKind, FieldKey, Function, Statement, StatementKind,
    Visitor, walk,
};
use crate::source::Source;

/// The hooks and the entity, weapon and panel methods that run every frame or tick, or often
/// enough that expensive work in them is noticeable.
pub const DEFAULT_HOT_PATHS: &[&str] = &[
    "CalcAbsolutePosition",
    "CalcMainActivity",
    "CalcVehicleView",
    "CalcView",
    "CalcViewModelView",
    "CreateMove",
    "Draw",
    "DrawDeathNotice",
    "DrawHUD",
    "DrawMonitors",
    "DrawOverlay",
    "DrawPhysgunBeam",
    "DrawTranslucent",
    "DrawWeaponSelection",
    "DrawWorldModel",
    "DrawWorldModelTranslucent",
    "FinishMove",
    "GetMotionBlurValues",
    "HUDAmmoPickedUp",
    "HUDDrawPickupHistory",
    "HUDDrawScoreBoard",
    "HUDDrawTargetID",
    "HUDItemPickedUp",
    "HUDPaint",
    "HUDPaintBackground",
    "HUDShouldDraw",
    "HUDWeaponPickedUp",
    "Move",
    "NeedsDepthPass",
    "Paint",
    "PaintOver",
    "PhysicsSimulate",
    "PhysicsUpdate",
    "PlayerDeathThink",
    "PlayerPostThink",
    "PlayerTick",
    "PostDraw2DSkyBox",
    "PostDrawEffects",
    "PostDrawHUD",
    "PostDrawOpaqueRenderables",
    "PostDrawPlayerHands",
    "PostDrawSkyBox",
    "PostDrawTranslucentRenderables",
    "PostDrawViewModel",
    "PostPlayerDraw",
    "PostRender",
    "PreDrawEffects",
    "PreDrawHalos",
    "PreDrawHUD",
    "PreDrawOpaqueRenderables",
    "PreDrawPlayerHands",
    "PreDrawSkyBox",
    "PreDrawTranslucentRenderables",
    "PreDrawViewModel",
    "PreDrawViewModels",
    "PrePlayerDraw",
    "PreRender",
    "RenderScene",
    "RenderScreenspaceEffects",
    "SetupMove",
    "SetupSkyboxFog",
    "SetupWorldFog",
    "ShouldDrawLocalPlayer",
    "StartCommand",
    "Think",
    "Tick",
    "UpdateAnimation",
    "VehicleMove",
    "ViewModelDrawn",
];

/// The functions registering a hook handler, called as `f(event, identifier, handler)`.
pub const DEFAULT_HOOK_FUNCTIONS: &[&str] = &["hook.Add"];

inventory::submit! {
    SharedOptions {
        department: "Performance",
        defaults: || vec![
            ("HotPaths", yaml_strings(DEFAULT_HOT_PATHS)),
            ("HookFunctions", yaml_strings(DEFAULT_HOOK_FUNCTIONS)),
        ],
    }
}

/// The calls inside the hot paths of `block` under the `HotPaths` and `HookFunctions` options
/// of the running reader; see [`hot_calls`].
pub fn configured_hot_calls<'t>(ctx: &Context, block: &'t Block) -> Vec<HotCall<'t>> {
    let hot_paths = ctx.option_str_list("HotPaths", DEFAULT_HOT_PATHS);
    let hook_functions = ctx.option_str_list("HookFunctions", DEFAULT_HOOK_FUNCTIONS);
    hot_calls(block, ctx.source, &hot_paths, &hook_functions)
}

/// A call of a plain function made inside a hot path.
#[derive(Debug, Clone, PartialEq)]
pub struct HotCall<'t> {
    /// The dotted name of the called function (`file.Read`).
    pub function: String,
    /// The arguments of the call.
    pub arguments: &'t Arguments,
    /// The bytes of the whole call.
    pub range: Range<usize>,
    /// The name of the innermost hot path the call is in (`HUDPaint`).
    pub hot_path: String,
}

/// The calls of plain functions (`f()`, `a.b()`) inside the hot paths of `block`, in source
/// order: the functions defined under one of the `hot_paths` names or registered for one of
/// them through a call of one of the `hook_functions`.
pub fn hot_calls<'t>(
    block: &'t Block,
    source: &Source,
    hot_paths: &[String],
    hook_functions: &[String],
) -> Vec<HotCall<'t>> {
    let mut collector = Collector {
        source,
        hot: hot_function_offsets(block, source, hot_paths, hook_functions),
        stack: Vec::new(),
        calls: Vec::new(),
    };
    walk(block, &mut collector);
    collector.calls
}

/// The hot functions of `block` under the `HotPaths` and `HookFunctions` options of the running
/// reader; see [`hot_functions`].
pub fn configured_hot_functions<'t>(ctx: &Context, block: &'t Block) -> Vec<HotFunction<'t>> {
    let hot_paths = ctx.option_str_list("HotPaths", DEFAULT_HOT_PATHS);
    let hook_functions = ctx.option_str_list("HookFunctions", DEFAULT_HOOK_FUNCTIONS);
    hot_functions(block, ctx.source, &hot_paths, &hook_functions)
}

/// A hot function that is not nested in another one.
#[derive(Debug, Clone, PartialEq)]
pub struct HotFunction<'t> {
    /// The function; the functions nested in its body are part of the hot path.
    pub function: &'t Function,
    /// The name of the hot path it is (`HUDPaint`).
    pub hot_path: String,
}

/// The outermost hot functions of `block`, in source order: the functions defined under one of
/// the `hot_paths` names or registered for one of them through a call of one of the
/// `hook_functions`, other than those nested in such a function (which belong to it).
pub fn hot_functions<'t>(
    block: &'t Block,
    source: &Source,
    hot_paths: &[String],
    hook_functions: &[String],
) -> Vec<HotFunction<'t>> {
    let mut outermost = Outermost {
        hot: hot_function_offsets(block, source, hot_paths, hook_functions),
        depth: 0,
        functions: Vec::new(),
    };
    walk(block, &mut outermost);
    outermost.functions
}

/// The hot path of every hot function of `block`, keyed by the offset of its `function`
/// keyword.
fn hot_function_offsets(
    block: &Block,
    source: &Source,
    hot_paths: &[String],
    hook_functions: &[String],
) -> HashMap<usize, String> {
    let mut marker = Marker {
        source,
        hot_paths,
        hook_functions,
        hot: HashMap::new(),
        named: HashMap::new(),
        handlers: Vec::new(),
    };
    walk(block, &mut marker);
    for (name, hot_path) in marker.handlers {
        for offset in marker.named.get(&name).into_iter().flatten() {
            marker
                .hot
                .entry(*offset)
                .or_insert_with(|| hot_path.clone());
        }
    }
    marker.hot
}

/// Collects the hot functions that are not nested in another hot function.
struct Outermost<'t> {
    hot: HashMap<usize, String>,
    /// How many functions the walk is inside of, counted from the outermost hot one.
    depth: usize,
    functions: Vec<HotFunction<'t>>,
}

impl<'t> Visitor<'t> for Outermost<'t> {
    fn enter_function(&mut self, function: &'t Function) {
        if self.depth > 0 {
            self.depth += 1;
        } else if let Some(hot_path) = self.hot.get(&function.keyword.start) {
            self.functions.push(HotFunction {
                function,
                hot_path: hot_path.clone(),
            });
            self.depth = 1;
        }
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.depth = self.depth.saturating_sub(1);
    }
}

/// Finds the hot functions, keyed by the offset of their `function` keyword, and the functions
/// defined under a bare name, which a hook registration may refer to.
struct Marker<'a> {
    source: &'a Source,
    hot_paths: &'a [String],
    hook_functions: &'a [String],
    hot: HashMap<usize, String>,
    named: HashMap<String, Vec<usize>>,
    handlers: Vec<(String, String)>,
}

impl<'a> Marker<'a> {
    /// The hot path `name` stands for, if it is one.
    fn hot_path(&self, name: &str) -> Option<String> {
        self.hot_paths
            .iter()
            .find(|hot_path| hot_path.as_str() == name)
            .cloned()
    }

    /// Marks `function` as the body of `hot_path`.
    fn mark(&mut self, function: &Function, hot_path: String) {
        self.hot.insert(function.keyword.start, hot_path);
    }

    /// Marks the anonymous function `value` as the body of `hot_path`, if it is a function.
    fn mark_value(&mut self, value: &Expression, hot_path: String) {
        if let ExpressionKind::Function(function) = &value.kind {
            self.mark(function, hot_path);
        }
    }

    /// Records that the function `value` (if it is one) is defined under the bare `name`.
    fn define(&mut self, name: &str, value: &Expression) {
        if let ExpressionKind::Function(function) = &value.kind {
            self.named
                .entry(name.to_owned())
                .or_default()
                .push(function.keyword.start);
        }
    }

    /// The name an assignment target or table key stands for: `x.Name`, `x['Name']`.
    fn target_name(&self, target: &Expression) -> Option<&'a str> {
        match &target.kind {
            ExpressionKind::Field { name, .. } => Some(self.source.text_of(name)),
            ExpressionKind::Index { key, .. } => key.string_contents(self.source),
            _ => None,
        }
    }
}

impl<'t> Visitor<'t> for Marker<'_> {
    fn statement(&mut self, statement: &'t Statement) {
        match &statement.kind {
            StatementKind::Function { name, function } => {
                let last = name.method.as_ref().or(name.path.last());
                let Some(last) = last else {
                    return;
                };
                let text = self.source.text_of(last);
                if name.method.is_none() && name.path.len() == 1 {
                    self.named
                        .entry(text.to_owned())
                        .or_default()
                        .push(function.keyword.start);
                } else if let Some(hot_path) = self.hot_path(text) {
                    self.mark(function, hot_path);
                }
            }
            StatementKind::LocalFunction { name, function, .. } => {
                let text = self.source.text_of(name).to_owned();
                self.named
                    .entry(text)
                    .or_default()
                    .push(function.keyword.start);
            }
            StatementKind::Local { names, values, .. } => {
                for (name, value) in names.iter().zip(values) {
                    self.define(self.source.text_of(name), value);
                }
            }
            StatementKind::Assignment {
                targets, values, ..
            } => {
                for (target, value) in targets.iter().zip(values) {
                    if let ExpressionKind::Name(name) = &target.kind {
                        self.define(self.source.text_of(name), value);
                    } else if let Some(hot_path) =
                        self.target_name(target).and_then(|key| self.hot_path(key))
                    {
                        self.mark_value(value, hot_path);
                    }
                }
            }
            _ => {}
        }
    }

    fn expression(&mut self, expression: &'t Expression) {
        match &expression.kind {
            ExpressionKind::Table(table) => {
                for field in &table.fields {
                    let key = match &field.key {
                        FieldKey::Named(name) => Some(self.source.text_of(name)),
                        FieldKey::Bracketed(key) => key.string_contents(self.source),
                        FieldKey::Positional => None,
                    };
                    if let Some(hot_path) = key.and_then(|key| self.hot_path(key)) {
                        self.mark_value(&field.value, hot_path);
                    }
                }
            }
            ExpressionKind::Call { .. } => {
                let Some((function, arguments)) = expression.as_call(self.source) else {
                    return;
                };
                if !self.hook_functions.contains(&function) {
                    return;
                }
                let Some(hot_path) = arguments
                    .values
                    .first()
                    .and_then(|event| event.string_contents(self.source))
                    .and_then(|event| self.hot_path(event))
                else {
                    return;
                };
                match arguments.values.get(2).map(|handler| &handler.kind) {
                    Some(ExpressionKind::Function(function)) => self.mark(function, hot_path),
                    Some(ExpressionKind::Name(name)) => {
                        let name = self.source.text_of(name).to_owned();
                        self.handlers.push((name, hot_path));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Collects the calls inside the hot functions, tracking the innermost hot path around the
/// function being walked.
struct Collector<'a, 't> {
    source: &'a Source,
    hot: HashMap<usize, String>,
    stack: Vec<Option<String>>,
    calls: Vec<HotCall<'t>>,
}

impl<'t> Visitor<'t> for Collector<'_, 't> {
    fn enter_function(&mut self, function: &'t Function) {
        let hot_path = self
            .hot
            .get(&function.keyword.start)
            .cloned()
            .or_else(|| self.stack.last().cloned().flatten());
        self.stack.push(hot_path);
    }

    fn leave_function(&mut self, _function: &'t Function) {
        self.stack.pop();
    }

    fn expression(&mut self, expression: &'t Expression) {
        let Some(Some(hot_path)) = self.stack.last() else {
            return;
        };
        if let Some((function, arguments)) = expression.as_call(self.source) {
            self.calls.push(HotCall {
                function,
                arguments,
                range: expression.range.clone(),
                hot_path: hot_path.clone(),
            });
        }
    }
}

/// Whether `expression` is a constant: a literal, or parentheses and operators over constants.
pub fn is_constant(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Nil
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::Number
        | ExpressionKind::String(_) => true,
        ExpressionKind::Parenthesized(inner) => is_constant(inner),
        ExpressionKind::Unary { operand, .. } => is_constant(operand),
        ExpressionKind::Binary { left, right, .. } => is_constant(left) && is_constant(right),
        ExpressionKind::Ternary {
            condition,
            consequent,
            alternative,
            ..
        } => is_constant(condition) && is_constant(consequent) && is_constant(alternative),
        _ => false,
    }
}

/// Whether `expression` can be written twice without changing what the code does or costing a
/// call: a name, or fields and indexes over a name whose keys are literals or names (`t`,
/// `self.items`, `data[id].list`).
pub fn is_repeatable(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Name(_) => true,
        ExpressionKind::Field { object, .. } => is_repeatable(object),
        ExpressionKind::Index { object, key } => {
            matches!(
                key.kind,
                ExpressionKind::Name(_)
                    | ExpressionKind::Number
                    | ExpressionKind::String(_)
                    | ExpressionKind::True
                    | ExpressionKind::False
            ) && is_repeatable(object)
        }
        _ => false,
    }
}

/// Longest code quoted in a message; longer code is shown as `...`.
const MAX_QUOTED: usize = 40;

/// `code` as quoted in a message: itself when short and on one line, otherwise `...`.
pub fn abbreviated(code: &str) -> &str {
    if code.len() <= MAX_QUOTED && !code.contains('\n') {
        code
    } else {
        "..."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::lint::ast::parse;

    /// The `(function, hot path)` pairs of the hot calls in `src`.
    fn calls(src: &str) -> Vec<(String, String)> {
        let source = Source::new("test.lua", src);
        let block = parse(&source).expect("parses");
        let hot_paths: Vec<String> = DEFAULT_HOT_PATHS.iter().map(|s| (*s).to_owned()).collect();
        let hooks: Vec<String> = DEFAULT_HOOK_FUNCTIONS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        hot_calls(&block, &source, &hot_paths, &hooks)
            .into_iter()
            .map(|call| (call.function, call.hot_path))
            .collect()
    }

    #[test]
    fn finds_calls_in_methods_fields_and_hooks() {
        assert_eq!(
            calls("function GM:Think() a() end\nfunction GM:Init() b() end\n"),
            vec![("a".to_owned(), "Think".to_owned())]
        );
        assert_eq!(
            calls("PANEL.Paint = function(self) a() end\nt['Draw'] = function() b() end\n"),
            vec![
                ("a".to_owned(), "Paint".to_owned()),
                ("b".to_owned(), "Draw".to_owned())
            ]
        );
        assert_eq!(
            calls("local p = { Paint = function() a() end, Init = function() b() end }\n"),
            vec![("a".to_owned(), "Paint".to_owned())]
        );
        assert_eq!(
            calls(
                "hook.Add('HUDPaint', 'x', function() a() end)\nhook.Add('Init', 'x', function() b() end)\n"
            ),
            vec![("a".to_owned(), "HUDPaint".to_owned())]
        );
    }

    #[test]
    fn resolves_named_handlers_and_nested_functions() {
        assert_eq!(
            calls(
                "hook.Add('Tick', 'x', tick)\nlocal function tick() a() end\nfunction other() b() end\n"
            ),
            vec![("a".to_owned(), "Tick".to_owned())]
        );
        assert_eq!(
            calls("function GM:Think() pcall(function() a() end) end\n"),
            vec![
                ("pcall".to_owned(), "Think".to_owned()),
                ("a".to_owned(), "Think".to_owned())
            ]
        );
        assert_eq!(calls("function Think() a() end\n"), Vec::new());
    }

    /// The `(first line, hot path)` pairs of the outermost hot functions in `src`.
    fn functions(src: &str) -> Vec<(u32, String)> {
        let source = Source::new("test.lua", src);
        let block = parse(&source).expect("parses");
        let hot_paths: Vec<String> = DEFAULT_HOT_PATHS.iter().map(|s| (*s).to_owned()).collect();
        let hooks: Vec<String> = DEFAULT_HOOK_FUNCTIONS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        hot_functions(&block, &source, &hot_paths, &hooks)
            .into_iter()
            .map(|hot| (source.position(hot.function.keyword.start).0, hot.hot_path))
            .collect()
    }

    #[test]
    fn finds_outermost_hot_functions() {
        assert_eq!(
            functions(
                "function GM:Think()\n  local p = { Paint = function() end }\nend\n\
                 function GM:Init() end\nhook.Add('Tick', 'x', tick)\nlocal function tick() end\n"
            ),
            vec![(1, "Think".to_owned()), (6, "Tick".to_owned())]
        );
    }

    #[test]
    fn repeatable_expressions() {
        let source = Source::new(
            "test.lua",
            "f(t, self.items, a[1].b, a[k], a['x'], f().x, a[k + 1], (t), a:b(), t[true])\n",
        );
        let block = parse(&source).expect("parses");
        let StatementKind::Call(call) = &block.statements[0].kind else {
            panic!("call");
        };
        let (_, arguments) = call.as_call(&source).expect("call");
        let repeatable: Vec<bool> = arguments.values.iter().map(is_repeatable).collect();
        assert_eq!(
            repeatable,
            vec![
                true, true, true, true, true, false, false, false, false, true
            ]
        );
        assert_eq!(abbreviated("x"), "x");
        assert_eq!(abbreviated("{\n}"), "...");
    }

    #[test]
    fn constants() {
        let source = Source::new("test.lua", "f(1, 'a', -2, (3 * 4), nil, x, g(), 1 + y)\n");
        let block = parse(&source).expect("parses");
        let StatementKind::Call(call) = &block.statements[0].kind else {
            panic!("call");
        };
        let (_, arguments) = call.as_call(&source).expect("call");
        let constant: Vec<bool> = arguments.values.iter().map(is_constant).collect();
        assert_eq!(
            constant,
            vec![true, true, true, true, true, false, false, false]
        );
    }
}
