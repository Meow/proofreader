//! Autocorrects Lumen templates with every reader at once and checks that the markup comes out
//! byte for byte as it went in, while the Lua around and inside it is corrected.

use std::path::Path;

use proofreader::config::Config;
use proofreader::runner::{Options, ReaderFilter, inspect_source};
use proofreader::source::Source;
use proofreader::token::TokenKind;

/// A template indented with four spaces, with style problems in its Lua, in the Lua spliced
/// into its markup, and trailing whitespace after markup.
const MESSY: &str = r#"--- The card of an item.
-- @param props [Map title (String), items (List), footer (String)]
local function Card(props)
    local title = props.title~=nil and props.title or "Untitled";
    local count = #props.items
    return <view style={{gap=4}} color="red" title='it"s' wide>
        <text style={{ font = "text_normal" }}>{title}  ({count}) </text>
        {count > 0 && Lumen.map(props.items, function(item)
            return <Row key={item.id} item={item} label="a  b"/>
        end)}
        <!-- a comment with   double spaces and "quotes" -->
        {  props.footer  }  	
        text with "quotes", 'apostrophes', end and -- not a comment
        <Flux.Panels.Card title='x' {...props}>{-count}</Flux.Panels.Card>
    </view>
end

return function(props)
  if not props then return <view/> end
  return <Card title={props.title} items={props.items or {}} footer="x" />
end
"#;

/// The texts of the markup tokens of a template.
fn markup(text: &str) -> Vec<String> {
    let source = Source::new("card.lumen", text);
    source
        .tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Markup)
        .map(|token| source.text_of(token).to_owned())
        .collect()
}

/// `text` autocorrected with every reader, and the offenses left afterwards.
fn correct(text: &str) -> (String, Vec<proofreader::offense::Offense>) {
    let config = Config::defaults(Path::new("."));
    let options = Options {
        fix: true,
        filter: ReaderFilter::default(),
        ..Options::default()
    };
    let (_, corrected) = inspect_source(&Source::new("card.lumen", text), &config, &options);
    let corrected = corrected.unwrap_or_else(|| text.to_owned());
    let (left, again) = inspect_source(
        &Source::new("card.lumen", corrected.as_str()),
        &config,
        &options,
    );
    assert!(
        again.is_none(),
        "correction is not idempotent:\n{corrected}"
    );
    (corrected, left)
}

#[test]
fn corrections_never_touch_markup() {
    let (corrected, left) = correct(MESSY);
    assert_eq!(
        markup(&corrected),
        markup(MESSY),
        "markup changed:\n{corrected}"
    );
    assert!(
        left.iter().all(|offense| !offense.correctable()),
        "correctable offenses remain: {:?}",
        left.iter()
            .map(|offense| (offense.reader, offense.line, &offense.message))
            .collect::<Vec<_>>()
    );
    assert!(corrected.contains("local title = props.title != nil and props.title or 'Untitled'\n"));
    assert!(
        corrected
            .contains("  return <view style={{ gap = 4 }} color=\"red\" title='it\"s' wide>\n")
    );
    assert!(
        corrected.contains(
            "\n      <text style={{ font = 'text_normal' }}>{title}  ({count}) </text>\n"
        )
    );
    assert!(corrected.contains("\n      {count > 0 and Lumen.map(props.items, function(item)\n"));
    assert!(
        corrected.contains("\n          return <Row key={item.id} item={item} label=\"a  b\"/>\n")
    );
    assert!(corrected.contains("\n      {  props.footer  }\n"));
    assert!(corrected.contains("\n      end)}\n"));
    assert!(corrected.contains("\n  </view>\nend\n"));
    assert!(corrected.contains("  if !props then return <view/> end\n"));
}

#[test]
fn a_clean_template_has_no_offenses() {
    let (corrected, _) = correct(MESSY);
    let config = Config::defaults(Path::new("."));
    let (offenses, _) = inspect_source(
        &Source::new("card.lumen", corrected.as_str()),
        &config,
        &Options::default(),
    );
    assert!(
        offenses.is_empty(),
        "offenses in the corrected template:\n{corrected}\n{:?}",
        offenses
            .iter()
            .map(|offense| (offense.reader, offense.line, offense.col, &offense.message))
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "reads the Flux corpus from /home/luna/code/flux-ce"]
fn corrections_keep_the_markup_of_the_flux_templates() {
    let root = Path::new("/home/luna/code/flux-ce");
    let mut templates = 0;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lumen"))
    {
        let text = std::fs::read_to_string(entry.path()).expect("UTF-8 template");
        let messy: String = text
            .lines()
            .map(|line| {
                let indent = line.len() - line.trim_start().len();
                format!("{}{}\n", " ".repeat(indent * 2), line.trim_start())
            })
            .collect();
        for input in [text.as_str(), messy.as_str()] {
            let (corrected, left) = correct(input);
            assert_eq!(
                markup(&corrected),
                markup(input),
                "{}: markup changed",
                entry.path().display()
            );
            assert!(
                left.iter().all(|offense| !offense.correctable()),
                "{}: correctable offenses remain",
                entry.path().display()
            );
        }
        templates += 1;
    }
    assert!(templates >= 5, "only {templates} templates found");
}
