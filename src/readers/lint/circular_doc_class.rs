//! `Lint/CircularDocClass`.

use std::collections::HashMap;

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::doc_annotations::{annotations, type_declaration};

/// Flags an EmmyLua class that inherits from itself through the parents declared in the file
/// (`---@class A : B` and `---@class B : A`), which leaves a language server unable to resolve
/// either class's members.
///
/// Parents are the plain names after the `:` of `---@class` and `---@interface` headers (generic
/// parents such as `List<T>` are left out); several declarations of one class add up. Each class
/// on a cycle is reported at the declaration that names a parent leading back to it.
pub struct CircularDocClass;

impl Reader for CircularDocClass {
    fn name(&self) -> &'static str {
        "Lint/CircularDocClass"
    }

    fn description(&self) -> &'static str {
        "Checks for EmmyLua classes that inherit from themselves."
    }

    fn investigate(&self, ctx: &mut Context) {
        let found = annotations(ctx.source);
        let declarations: Vec<_> = found
            .iter()
            .filter_map(type_declaration)
            .filter(|declaration| !declaration.parents.is_empty())
            .collect();
        let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
        for declaration in &declarations {
            parents
                .entry(declaration.name)
                .or_default()
                .extend(declaration.parents.iter().map(|(name, _)| *name));
        }
        for declaration in &declarations {
            for (parent, range) in &declaration.parents {
                if let Some(path) = path_to(&parents, parent, declaration.name) {
                    let chain: Vec<&str> = std::iter::once(declaration.name).chain(path).collect();
                    ctx.add_offense(
                        range.clone(),
                        format!(
                            "Class `{}` inherits from itself: {}.",
                            declaration.name,
                            chain.join(" → ")
                        ),
                    );
                    break;
                }
            }
        }
    }
}

/// The chain of classes from `from` up to `target` through `parents` (both ends included), if
/// `target` is an ancestor of `from` or `from` itself.
fn path_to<'a>(
    parents: &HashMap<&'a str, Vec<&'a str>>,
    from: &'a str,
    target: &str,
) -> Option<Vec<&'a str>> {
    let mut stack = vec![vec![from]];
    let mut visited = vec![from];
    while let Some(path) = stack.pop() {
        let last = path[path.len() - 1];
        if last == target {
            return Some(path);
        }
        for parent in parents.get(last).into_iter().flatten() {
            if visited.contains(parent) {
                if *parent == target {
                    let mut found = path.clone();
                    found.push(parent);
                    return Some(found);
                }
                continue;
            }
            visited.push(parent);
            let mut next = path.clone();
            next.push(parent);
            stack.push(next);
        }
    }
    None
}

inventory::submit! { Registration(|| Box::new(CircularDocClass)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Lint/CircularDocClass";

    #[test]
    fn flags_classes_on_a_cycle() {
        expect_offenses(
            READER,
            "---@class A : B\n---@class B : C\n---@class C : A, D\n---@class D\n---@class E : E\n",
            &[
                (1, 15, "Class `A` inherits from itself: A → B → C → A."),
                (2, 15, "Class `B` inherits from itself: B → C → A → B."),
                (3, 15, "Class `C` inherits from itself: C → A → B → C."),
                (5, 15, "Class `E` inherits from itself: E → E."),
            ],
        );
    }

    #[test]
    fn accepts_hierarchies_without_cycles() {
        expect_no_offenses(
            READER,
            "---@class A : B, C\n---@class B : C\n---@class C : Panel\n---@class L : List<L>\n\
             -- @class X : X\n",
        );
    }
}
