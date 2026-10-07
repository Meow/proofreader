# proofreader

A RuboCop-like linter and autocorrector for **GLua** (Garry's Mod Lua), encoding the code style of the Flux
framework. It understands the GLua extensions (`!`, `!=`, `&&`, `||`,
`continue`, `//` and `/* */` comments) and inspects hundreds of files in a few milliseconds.

## Usage

```
proofreader [PATHS]... [OPTIONS]            PATHS default to "."
  -a, --fix, --autocorrect       Fix correctable offenses in place
      --diff                     With --fix: print the changes as a diff instead of writing files
      --only <READERS>           Comma-separated reader names or departments
      --except <READERS>         Comma-separated reader names or departments to skip
  -f, --format <FORMAT>          progress (default) | offenses | files | quiet | json
  -c, --config <PATH>            Use this configuration file for every target
      --show-readers [PATTERN]   List readers; with PATTERN also their effective configuration
      --fail-level <SEVERITY>    Exit 1 only for offenses at or above this severity (default: info)
      --no-color                 Disable colors (also disabled by NO_COLOR or when stdout is not a terminal)
  -L, --list-target-files        Print the files that would be inspected and exit
  -v, --version / -h, --help
```

Directories are walked recursively (hidden directories are skipped) and filtered by `AllReaders`
`Include`/`Exclude`. Files given explicitly are always inspected, even when excluded or not ending in `.lua`.

Reader names in `--only`, `--except` and `--show-readers` are case-insensitive and may be a full name
(`Layout/LineLength`), a department (`Layout`) or a bare name (`LineLength`). A reader named explicitly in
`--only` runs even when its configuration disables it.

```
$ proofreader lib
Inspecting 3 files
.C.

Offenses:

lib/foo.lua:12:121: C: Layout/LineLength: Line is too long. [130/120]
  local message = ...
                                                                                                                        ^^^^^^^^^^
lib/foo.lua:14:12: C: [Correctable] Layout/TrailingWhitespace: Trailing whitespace detected.
  local a = 1  
             ^^

3 files inspected, 2 offenses detected, 1 offense autocorrectable
```

With `--fix`, fixed offenses are marked `[Corrected]` and the summary reports how many were corrected.
Autocorrection runs up to 10 passes per file, re-inspecting after each pass, so fixes that conflict in one pass
are applied in the next.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | No offenses at or above `--fail-level` remain (corrected offenses do not count). |
| 1 | At least one uncorrected offense at or above `--fail-level`. |
| 2 | Usage error, unknown reader, missing path, broken configuration or I/O error. |

### Formats

- `progress`: one symbol per file (`.` clean, otherwise the letter of the worst severity: `I`, `R`, `C`, `W`,
  `E`, `F`), then every offense with its source line and carets, then the summary.
- `quiet`: like `progress` without the progress line; prints nothing for a clean run.
- `offenses`: offense counts per reader, most frequent first, and a total.
- `files`: the paths of the files with offenses.
- `json`: `{ "metadata": {...}, "files": [{ "path", "offenses": [...] }], "summary": {...} }`; every offense has
  `severity`, `message`, `reader_name`, `corrected`, `correctable` and a `location`.

## Configuration

Each file is checked with the nearest `.proofreader.yml` found in its directory or one of its ancestors
(`--config` forces one file for everything). Without any configuration file, the built-in defaults apply.

```yaml
inherit_from: ../.proofreader.yml        # a string or a list, relative to this file

AllReaders:
  Include:                                # default: ['**/*.lua']
    - '**/*.lua'
  Exclude:
    - 'packages/pon/**/*'
  Severity: convention                    # default severity of readers without their own

Layout/LineLength:
  Enabled: true                           # default: true
  Severity: warning                       # info, refactor, convention, warning, error, fatal
  AutoCorrect: true                       # default: true
  Exclude: ['lib/generated.lua']          # reader-level file selection
  Max: 100                                # reader-specific options
```

- Merge order: built-in defaults, then the inherited files in order (each after its own `inherit_from`), then the
  file itself. Reader sections merge key by key; `Include` and `Exclude` lists replace earlier ones.
- Patterns are relative to the directory of the file that declares them. `**` matches any number of directories,
  `*` and `?` never match `/`, and a pattern without wildcards also matches everything below that directory.
  Patterns starting with `**` match anywhere.
- An unknown reader section produces `Warning: unrecognized reader Foo/Bar found in <file>` on stderr; invalid
  values (an unknown severity, a non-boolean `Enabled`, a bad glob) are errors.
- `proofreader --show-readers Layout` prints the effective configuration of the matching readers as YAML.

## Readers

| Reader | Severity | Autocorrect | Options |
|--------|----------|-------------|---------|
| `Layout/LineLength` | convention | no | `Max: 120`, `AllowURI: true`, `IgnoreComments: false` |
| `Layout/TrailingWhitespace` | convention | yes | |

`AllowURI` accepts a line whose overrun is a URI starting before the limit and running to the end of the line;
`IgnoreComments` accepts a line whose overrun starts inside a comment. Trailing whitespace inside strings is
never reported.

## Adding a reader

Readers live in `src/readers/<department>/<snake_case_name>.rs`, one per file; the department modules pick up
new files automatically through `automod` (`build.rs` makes cargo notice added files), and `inventory` registers
the reader, so nothing else needs editing.

```rust
//! `Style/Not`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};
use crate::token::TokenKind;

/// Flags `not` in favour of `!`.
pub struct Not;

impl Reader for Not {
    fn name(&self) -> &'static str {
        "Style/Not"
    }

    fn description(&self) -> &'static str {
        "Checks for uses of `not` instead of `!`."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        for (index, token) in source.code_tokens().enumerate() {
            if token.kind != TokenKind::Not {
                continue;
            }
            let end = source.next_code(index).map_or(token.end, |next| next.start);
            ctx.add_offense_with_fix(
                token.range(),
                "Use `!` instead of `not`.",
                vec![Edit::replace(token.start..end, "!")],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(Not)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Style/Not";

    #[test]
    fn flags_not() {
        expect_offense(
            READER,
            "if not x then end\n",
            1,
            4,
            "Use `!` instead of `not`.",
        );
        expect_correction(READER, "if not x then end\n", "if !x then end\n");
    }
}
```

- `Reader::default_severity` is `Warning` for `Lint/*` and `Convention` otherwise; override it when needed.
- `Reader::default_options` lists the reader's options with their defaults (`Yaml::Integer`, `Yaml::Boolean`,
  `Yaml::String`, `Yaml::Array`); read them with `ctx.option_usize("Max", 120)` and friends.
- `ctx.source` gives the text, the lines and every token (trivia included) with byte ranges and 1-based
  line/column positions, plus helpers such as `code_tokens`, `token_at`, `first_code_token_on_line` and
  `lines_inside_multiline_tokens`.
- A fix is a list of `Edit`s (`replace`, `remove`, `insert`) over byte ranges of the current text. Fixes that
  overlap an already accepted fix are retried in the next pass, so keep each fix minimal.
- Test with `crate::testing`: `inspect`, `inspect_with` (options as YAML, e.g. `"Max: 80"`), `autocorrect`,
  `expect_no_offenses`, `expect_offense`, `expect_offenses` and `expect_correction` (which also checks that the
  correction is idempotent).
- Messages are short sentences ending with a period, with numbers in brackets where useful
  (`Line is too long. [130/120]`).
- Shared helpers: `readers::lint::libraries::GMOD_LIBRARIES`, `readers::naming::case::is_lower_camel_case` and
  `readers::style::quotes::to_single_quoted`.

## Development

```
cargo test                                   # unit and integration tests
cargo test -- --ignored                      # also lex the whole Flux corpus at /home/luna/code/flux-ce
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
