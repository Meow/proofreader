//! `Lint/UnknownDocTag`.

use yaml_rust2::Yaml;

use crate::reader::{Context, Reader, Registration};
use crate::readers::style::doc_comments::tag;

/// Tags known by default: EmmyLua/LuaLS annotations, LDoc tags (with its `@tparam` aliases),
/// GLuaLS annotations and the tags of the Flux documentation generator.
pub const KNOWN_TAGS: &[&str] = &[
    "accessorfunc",
    "alias",
    "as",
    "async",
    "author",
    "bool",
    "cast",
    "category",
    "class",
    "classmod",
    "copyright",
    "deprecated",
    "deprecation",
    "deprecation_version",
    "diagnostic",
    "enum",
    "environment",
    "export",
    "field",
    "file",
    "fixme",
    "func",
    "function",
    "generic",
    "hook",
    "ignore",
    "int",
    "internal",
    "lfunction",
    "license",
    "local",
    "meta",
    "module",
    "nodiscard",
    "number",
    "operator",
    "overload",
    "package",
    "param",
    "private",
    "protected",
    "public",
    "raise",
    "realm",
    "release",
    "return",
    "script",
    "section",
    "see",
    "source",
    "string",
    "submodule",
    "tab",
    "table",
    "thread",
    "todo",
    "topic",
    "tparam",
    "treturn",
    "type",
    "usage",
    "vararg",
    "variant",
    "version",
    "warning",
    "within",
];

/// Flags `@tags` that are not in `KnownTags`, in any `--` line comment whose text starts with
/// a tag (`-- @param`, `---@class`). Disabled by default.
pub struct UnknownDocTag;

impl Reader for UnknownDocTag {
    fn name(&self) -> &'static str {
        "Lint/UnknownDocTag"
    }

    fn description(&self) -> &'static str {
        "Checks for documentation tags that are not in `KnownTags`."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn default_options(&self) -> Vec<(&'static str, Yaml)> {
        vec![(
            "KnownTags",
            Yaml::Array(
                KNOWN_TAGS
                    .iter()
                    .map(|tag| Yaml::String((*tag).to_owned()))
                    .collect(),
            ),
        )]
    }

    fn investigate(&self, ctx: &mut Context) {
        let known = ctx.option_str_list("KnownTags", KNOWN_TAGS);
        let source = ctx.source;
        for comment in source.code_tokens() {
            let Some(tag) = tag(source, comment) else {
                continue;
            };
            if known.iter().any(|name| name == tag.name) {
                continue;
            }
            ctx.add_offense(
                tag.range.clone(),
                format!("Unknown documentation tag `@{}`.", tag.name),
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(UnknownDocTag)) }

#[cfg(test)]
mod tests {
    use super::KNOWN_TAGS;
    use crate::testing::*;

    const READER: &str = "Lint/UnknownDocTag";

    #[test]
    fn flags_unknown_tags() {
        expect_offenses(
            READER,
            "--- Does things.\n-- @parm a [Number]\n---@retrun number\nx = 1 -- @foo\n",
            &[
                (2, 4, "Unknown documentation tag `@parm`."),
                (3, 4, "Unknown documentation tag `@retrun`."),
                (4, 10, "Unknown documentation tag `@foo`."),
            ],
        );
    }

    #[test]
    fn accepts_known_tags_and_other_comments() {
        expect_no_offenses(
            READER,
            "---@class Foo\n-- @param a [Number]\n--   @variant f(a)\n-- @return [X]\n\
             -- mail me @ home\n-- user@example.com\n--[[ @foo ]]\ns = '-- @foo'\n",
        );
    }

    #[test]
    fn known_tags_are_configurable() {
        assert!(inspect_with(READER, "-- @player p\n", "KnownTags: [player]").is_empty());
        assert_eq!(
            inspect_with(READER, "-- @param p\n", "KnownTags: [player]").len(),
            1
        );
    }

    #[test]
    fn is_disabled_by_default() {
        let config = crate::config::Config::defaults(std::path::Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn default_list_is_sorted() {
        let mut sorted = KNOWN_TAGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, KNOWN_TAGS);
    }
}
