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

### Disabling readers inline

Comments switch readers off for part of a file, like ESLint's `eslint-disable` comments:

| Comment | Switches off offenses |
|---------|-----------------------|
| `-- proofreader-disable` | from the comment up to a matching `proofreader-enable` or the end of the file |
| `-- proofreader-enable` | none: ends an earlier `proofreader-disable` |
| `-- proofreader-disable-line` | on the line of the comment |
| `-- proofreader-disable-next-line` | on the line after the comment |

```lua
-- proofreader-disable Layout/LineLength, Style/Not
local message = not ok and 'a very long line ...'
-- proofreader-enable Style/Not

local Legacy_Name = 1 -- proofreader-disable-line Naming/VariableName

-- proofreader-disable-next-line Style -- generated by the item editor
local label = "it's \"fine\"";
```

- Without a reader list a directive applies to every reader. Readers are separated by commas or spaces and
  named as in `--only`: a full name, a department or a bare name, case-insensitively. Unknown names are ignored.
- Text after a `--` standing on its own is an explanation.
- Any comment form works: `--`, `//`, `--[[ ]]` and `/* */`.
- An offense is switched off when it *starts* in a disabled range or on a disabled line (the position shown in
  the report). A `disable` or `enable` comment on a line of its own takes effect from the start of that line,
  otherwise from the comment itself, so `/* proofreader-disable */ ... /* proofreader-enable */` can wrap part
  of a line.
- Disabled offenses are neither reported nor autocorrected, and do not count for the exit code.

## Readers

`proofreader --show-readers` lists every reader with its description (38 in total);
`proofreader --show-readers PATTERN` (a name, a department or a bare name) prints the effective
configuration of the matching readers as YAML, which is a good starting point for a
`.proofreader.yml`. Every reader also accepts `Enabled`, `Severity`, `AutoCorrect`, `Include` and
`Exclude`. "Fix" says whether the reader autocorrects with `--fix`.

### Layout

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Layout/ByteOrderMark` | No UTF-8 byte order mark at the start of the file. | convention | yes | |
| `Layout/EmptyLineAfterBlock` | A blank line after a line that closes a multi-line block with `end`, unless the next line is a closer (`end`, `else`, `elseif`, `until`, `}`, `)`), EOF, or returns to the level of an indented group. Lines ending in `,` (`end,`) are exempt. | convention | yes | |
| `Layout/EmptyLineAfterGuardClause` | A blank line after a one-line `if ... then return/continue/break ... end`, unless followed by a closer or another guard clause. | convention | yes | |
| `Layout/EmptyLineBeforeBlock` | A blank line before multi-line `if`/`for`/`while`/`repeat`/`do` statements and named function definitions, unless first in their block, first in an indented group, or after a comment. | convention | yes | `IncludeAnonymousFunctions: false` (also check statements opening a multi-line anonymous function, such as `btn.DoClick = function(b)`) |
| `Layout/EmptyLineBetweenDefs` | Blank lines between a function definition's `end` and the next definition (or its doc comment). | convention | yes | `NumberOfEmptyLines: 1`, `AllowAdjacentOneLineDefs: true` |
| `Layout/EmptyLines` | No two consecutive blank lines. | convention | yes | |
| `Layout/EmptyLinesAroundBlockBody` | No blank line directly after a line opening a block or directly before a closer line. | convention | yes | |
| `Layout/EndOfLine` | LF line endings only (reported once per file). | convention | yes | |
| `Layout/ExtraSpacing` | No runs of several spaces between tokens (gaps owned by other spacing readers excluded). | convention | yes | `AllowForAlignment: true` (accept a run when the next token lines up with a token on an adjacent line), `ForceEqualSignAlignment: false` (align the `=` of consecutive assignments) |
| `Layout/IndentationConsistency` | The statements of a block are indented like its first statement; column-aligned lines are accepted. | convention | yes | `AllowIndentedGroups: true` (a run of statements exactly `IndentationWidth` deeper that returns to the block's level afterwards), `IndentationWidth: 2` |
| `Layout/IndentationStyle` | Spaces, not tabs, in leading indentation. | convention | yes | `IndentationWidth: 2` (spaces per tab in the fix) |
| `Layout/IndentationWidth` | The first body line of a block is `Width` deeper than its opener line; closers align with the opener line. | convention | yes | `Width: 2` |
| `Layout/LeadingCommentSpace` | A space after `--`, `---` and `//` (block comments and dash-only lines exempt). | convention | yes | |
| `Layout/LineLength` | Lines are at most `Max` characters. The fix only changes whitespace and picks, per line, the first of these that fits: break after the `=` of an assignment or table field (value one level deeper); unfold the call argument list or table constructor holding the overrun (outermost first, a last table argument hugged as `f(a, {`): one item per line, the closer on its own line, lines of a trailing multi-line function re-indented; break a chain after its last `,`/`or`/`and`/`..` that fits (before the operator when the chain already leads with it; `if`/`elseif`/`while` conditions continue under the first condition), or before the `:` of a chained method call. A step that does not fit at once is taken when the following passes can finish it. Comment-only lines, overruns inside a trailing comment, aligned lines and lines without a break point stay uncorrected. | convention | yes | `Max: 120`, `AllowURI: true`, `IgnoreComments: false`, `IndentationWidth: 2` (indentation of inserted lines) |
| `Layout/SpaceAfterComma` | A space after every comma followed by code. | convention | yes | |
| `Layout/SpaceAfterNot` | No space between `!` and its operand. | convention | yes | |
| `Layout/SpaceAroundOperators` | One space around binary operators and `=`; none around `..`. Unary `-`, `#`, `!`, `not` are not binary. | convention | yes | `ConcatStyle: no_space` (or `space`), `AllowForAlignment: true` (extra spaces before an operator that lines up with the same operator on an adjacent line) |
| `Layout/SpaceBeforeComma` | No space before a comma. | convention | yes | |
| `Layout/SpaceBeforeComment` | A space between code and a trailing comment. | convention | yes | |
| `Layout/SpaceBeforeParen` | No space between a function name and its `(`. | convention | yes | |
| `Layout/SpaceInsideBraces` | `{ a = 1 }` with one space inside non-empty braces, `{}` when empty. | convention | yes | `EnforcedStyle: space` (or `no_space`) |
| `Layout/SpaceInsideBrackets` | No spaces inside `[` `]` of indexing and table keys. | convention | yes | |
| `Layout/SpaceInsideParens` | No spaces inside `(` `)`. | convention | yes | |
| `Layout/TrailingEmptyLines` | Exactly one `\n` at the end of the file. | convention | yes | |
| `Layout/TrailingWhitespace` | No trailing spaces or tabs (outside strings). | convention | yes | |

### Style

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Style/AndOr` | `and`/`or` instead of `&&`/`\|\|`. | convention | yes | |
| `Style/DocumentationMethod` | Every named function definition is directly preceded by a doc comment starting with `---`. | convention | no | `RequireForLocalFunctions: false` |
| `Style/InequalityOperator` | `!=` instead of `~=`. | convention | yes | |
| `Style/Not` | `!` instead of `not`. | convention | yes | |
| `Style/ParenthesesAroundCondition` | No parentheses wrapping the whole condition of `if`, `elseif`, `while` or `until`. | convention | yes | |
| `Style/Semicolon` | No semicolons (a `;` between table fields becomes a comma). | convention | yes | |
| `Style/StringLiterals` | Single quotes, unless the string contains a single quote; long strings exempt. | convention | yes | `EnforcedStyle: single_quotes` (or `double_quotes`) |
| `Style/TrailingCommaInTable` | No comma after the last item of a table. | convention | yes | `EnforcedStyle: no_comma` (or `comma`: required in multi-line tables) |

### Lint

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Lint/DuplicateTableKey` | The same literal key twice in one table constructor (`a = 1, a = 2`, `['a']`, `[1]`/`[1.0]`). | warning | no | |
| `Lint/ShadowedLibrary` | Locals, parameters and loop variables named after a GMod library (`local file = ...`); `local render = render` is allowed. | warning | no | `Libraries: [player, team, file, table, sound, string, math, util, net, hook, timer, render, surface, draw, ents, game, engine, input, gui, vgui, http, os, debug, bit]` |
| `Lint/Syntax` | Unterminated strings and block comments, stray characters, and the first unbalanced bracket, `end` or `until`. | fatal | no | |

### Naming

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Naming/MethodName` | Defined functions and methods (including `local function`) are not lowerCamelCase; PascalCase is allowed for GMod hooks. | convention | no | |
| `Naming/VariableName` | Locals, parameters and loop variables are not lowerCamelCase (snake_case, SCREAMING_CASE and ConstantStyle allowed). | convention | no | |

### Known setting conflict

`Layout/ExtraSpacing` with `ForceEqualSignAlignment: true` pads the `=` of consecutive
assignments into one column, while `Layout/SpaceAroundOperators` with `AllowForAlignment: false`
removes every extra space before an `=`. Enabling both makes the two readers undo each other on
every autocorrect pass (until the pass limit is reached); keep `AllowForAlignment: true` when
forcing alignment.

## Flux conventions

The defaults encode the style of the Flux framework:

- 2-space indentation, no tabs, no trailing whitespace, LF line endings, exactly one newline at
  the end of the file, lines of at most 120 characters.
- Single-quoted strings unless the string contains `'`; `!` and `!=` instead of `not` and `~=`;
  `and`/`or` instead of `&&`/`||`.
- `'a'..b..'c'`: no spaces around `..`, one space around every other binary operator and `=`.
- `{ a = 1, 'x' }` with spaces inside non-empty braces and `{}` when empty; no spaces inside
  parentheses or brackets; one space after a comma and none before; no space before a call's `(`.
- snake_case variables, functions and methods; ConstantStyle classes; GMod hooks keep PascalCase.
- Every function definition has a doc comment (`--- Summary.` followed by `-- @param` and
  `-- @return` lines).
- A blank line before every multi-line block statement and after its `end` (unless next to
  another opener or closer), after a one-line guard clause (`if !x then return end`) and between
  function definitions; no blank line right after an opener or right before `end`; never two
  blank lines in a row.
- No parentheses around conditions, no trailing commas in tables, no semicolons.
- Never name a variable after a GMod library (`player`, `file`, `table`, ...): use `actor`,
  `target`, `client`, `path`, `tbl` and the like.
- Statement groups may be indented one level deeper to show a scope, as long as the closing
  statement returns to the block's level:

  ```lua
  net.Start('flux_notify')
    net.WriteString(text)
  net.Send(target)
  ```

## Shared helper modules

Readers share a few helper modules, which are ordinary modules of their department:

| Module | Provides |
|--------|----------|
| `readers::layout::block_structure` | `BlockStructure`: per-line block nesting, line kinds (statement, continuation, closer, comment, blank, inside a multi-line token), opener/closer lines, guard clauses, definitions and doc comments, plus re-indent and blank-line insertion edits. Used by the indentation and blank-line readers. |
| `readers::layout::spacing` | The gap between adjacent code tokens, unary/binary operator classification and the gaps inside bracket pairs. Used by the token-spacing readers. |
| `readers::layout::line_breaking` | `LineBreaker`: plans the whitespace-only line breaks of `Layout/LineLength`'s fix, simulating follow-up passes to check that a line can be finished. |
| `readers::layout::alignment` | Character columns and the alignment checks behind `AllowForAlignment` and `ForceEqualSignAlignment`. |
| `readers::naming::declarations` | Local variables, parameters, loop variables and named function definitions of a file. |
| `readers::naming::case` | `is_lower_camel_case`. |
| `readers::lint::nesting` | A bracket and block-keyword tracker used by `Lint/Syntax` and `Lint/DuplicateTableKey`. |
| `readers::lint::libraries` | `GMOD_LIBRARIES`, the default list of `Lint/ShadowedLibrary`. |
| `readers::style::quotes` | Converting string literals between single and double quotes. |

## Adding a reader

Readers live in `src/readers/<department>/<snake_case_name>.rs`, one per file. Add the file **and**
a `pub mod <snake_case_name>;` line (kept sorted) to `src/readers/<department>/mod.rs`; `inventory`
then registers the reader, so nothing else needs editing.

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
- Code style: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, no `unsafe`, no `unwrap()` outside
  tests, and `///`/`//!` documentation comments only (no `//` comments).

## Development

```
cargo test                                   # unit and integration tests
cargo test -- --ignored                      # also run over the whole Flux corpus at /home/luna/code/flux-ce:
                                             # lexing, and checks that every correction (all readers together,
                                             # and the layout groups alone) keeps the code and is idempotent,
                                             # and that Layout/LineLength alone only breaks lines, shortens
                                             # them and adds no offenses of other readers
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

An example configuration showing the common options is in
[`examples/.proofreader.yml`](examples/.proofreader.yml).
