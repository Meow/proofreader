//! The example styles in `examples/`: every configuration loads cleanly, every example program
//! is clean under its own style, and autocorrecting the Flux example with a style's
//! configuration turns it into that style's example.
//!
//! Regenerate the converted examples with `PROOFREADER_BLESS=1 cargo test --test examples`
//! (and review the diff).

use std::path::{Path, PathBuf};

use proofreader::config::Config;
use proofreader::runner::{Options, inspect_source};
use proofreader::source::Source;

/// The example styles, by directory name under `examples/`.
const STYLES: [&str; 6] = [
    "flux",
    "catwork",
    "clockwork",
    "gmod",
    "helix",
    "stroustrup",
];

/// The directory of the style `name`.
fn style_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

/// The configuration of the style `name`, which must load without warnings.
fn style_config(name: &str) -> Config {
    let config = Config::load(&style_dir(name).join(".proofreader.yml"))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    assert!(
        config.warnings().is_empty(),
        "{name}: {:?}",
        config.warnings()
    );
    config
}

/// The example program of the style `name`.
fn example(name: &str) -> (PathBuf, String) {
    let path = style_dir(name).join("example.lua");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
    (path, text)
}

#[test]
fn every_example_is_clean_under_its_style() {
    for name in STYLES {
        let config = style_config(name);
        let (path, text) = example(name);
        let (offenses, _) = inspect_source(&Source::new(path, text), &config, &Options::default());
        let found: Vec<String> = offenses
            .iter()
            .map(|offense| {
                format!(
                    "{}:{}: {}: {}",
                    offense.line, offense.col, offense.reader, offense.message
                )
            })
            .collect();
        assert!(found.is_empty(), "{name}:\n{}", found.join("\n"));
    }
}

#[test]
fn autocorrecting_the_flux_example_yields_each_style() {
    let (_, flux) = example("flux");
    for name in STYLES {
        let config = style_config(name);
        let path = style_dir(name).join("example.lua");
        let options = Options {
            fix: true,
            ..Options::default()
        };
        let (_, corrected) =
            inspect_source(&Source::new(path.clone(), flux.clone()), &config, &options);
        let corrected = corrected.unwrap_or_else(|| flux.clone());
        if std::env::var_os("PROOFREADER_BLESS").is_some() && name != "flux" {
            std::fs::write(&path, &corrected).expect("write the example");
            continue;
        }
        let (_, expected) = example(name);
        assert_eq!(corrected, expected, "{name}");
    }
}
