# proofreader

A RuboCop-like linter and autocorrector for **GLua** (Garry's Mod Lua), encoding the code style of the Flux
framework. It understands the GLua extensions (`!`, `!=`, `&&`, `||`,
`continue`, `//` and `/* */` comments) and inspects hundreds of files in a few milliseconds.

Besides GLua files (`.lua`) it inspects Flux's Lumen templates (`.lumen`), GLua with JSX-like markup in
expression position; see [Lumen templates](#lumen-templates).

## Usage

```
proofreader [PATHS]... [OPTIONS]            PATHS default to "."
  -a, --fix, --autocorrect       Fix correctable offenses in place
      --diff                     With --fix: print the changes as a diff instead of writing files
      --stdin <PATH>             Inspect the source read from stdin as the file PATH
      --only <READERS>           Comma-separated reader names or departments
      --except <READERS>         Comma-separated reader names or departments to skip
  -f, --format <FORMAT>          progress (default) | offenses | files | quiet | json | sarif
  -c, --config <PATH>            Use this configuration file for every target
      --show-readers [PATTERN]   List readers; with PATTERN also their effective configuration
      --fail-level <SEVERITY>    Exit 1 only for offenses at or above this severity (default: info)
      --no-color                 Disable colors (also disabled by NO_COLOR or when stdout is not a terminal)
  -L, --list-target-files        Print the files that would be inspected and exit
  -v, --version / -h, --help
```

Directories are walked recursively (hidden directories are skipped) for `.lua` and `.lumen` files, filtered by
`AllReaders` `Include`/`Exclude`. Files given explicitly are always inspected, even when excluded or not
ending in `.lua` or `.lumen`; a file ending in `.lumen` is read as a Lumen template, any other as GLua.

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
Autocorrection runs up to 25 passes per file, re-inspecting after each pass, so fixes that conflict in one pass
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
- `sarif`: a SARIF 2.1.0 log for code scanning tools (GitHub code scanning and others): one rule per reader
  that reported, one result per uncorrected offense with `level` `error` (error, fatal), `warning` or
  `note`, a relative `uri` and a region in Unicode code point columns; files that could not be read or
  written become tool notifications.

### Standard input

`proofreader --stdin path/to/file.lua` inspects the source read from standard input as if it were that
file: its configuration is the one for that path (which need not exist), and a path ending in `.lumen`
makes it a Lumen template. No other paths may be given. The report uses the selected format. With `--fix`,
only the corrected source (or the unchanged source) is written to standard output, so editors can pipe a
buffer through `proofreader --stdin "$FILE" --fix`; `--fix --diff` prints the diff and the report instead.
The exit status is the usual one for the offenses that remain.

## Configuration

Each file is checked with the nearest `.proofreader.yml` found in its directory or one of its ancestors
(`--config` forces one file for everything). Without any configuration file, the built-in defaults apply.

```yaml
inherit_from: ../.proofreader.yml        # a string or a list, relative to this file

AllReaders:
  Include:                                # default: ['**/*.lua', '**/*.lumen']
    - '**/*.lua'
    - '**/*.lumen'
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
- A section named after a department (`Performance:`) applies to every reader of it: its `Enabled`,
  `Severity`, `AutoCorrect`, `Include` and `Exclude` are the defaults of those readers' own sections, and
  it holds the options the department's readers share (the hot paths of `Performance`), which a reader's
  own section may still override.
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

`proofreader --show-readers` lists every reader with its description (77 in total, 9 of them disabled by default);
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
| `Layout/EndOfLine` | Consistent line endings: LF, or CRLF with `EnforcedStyle: crlf` (reported once per file). Line endings inside markup are never changed. | convention | yes | `EnforcedStyle: lf` (or `crlf`) |
| `Layout/ExtraSpacing` | No runs of several spaces between tokens (gaps owned by other spacing readers excluded). | convention | yes | `AllowForAlignment: true` (accept a run when the next token lines up with a token on an adjacent line), `ForceEqualSignAlignment: false` (align the `=` of consecutive assignments) |
| `Layout/IndentationConsistency` | The statements of a block are indented like its first statement; column-aligned lines are accepted. | convention | yes | `AllowIndentedGroups: true` (a run of statements exactly `IndentationWidth` deeper that returns to the block's level afterwards), `IndentationWidth: 2` |
| `Layout/IndentationStyle` | Spaces, not tabs, in leading indentation; with `EnforcedStyle: tabs`, tabs for whole levels followed by fewer than `IndentationWidth` alignment spaces. `Layout/IndentationWidth` and `Layout/IndentationConsistency` count a tab as reaching the next multiple of their width (keep it equal to `IndentationWidth`); `Layout/LineLength` does not break tab-indented lines. | convention | yes | `EnforcedStyle: spaces` (or `tabs`), `IndentationWidth: 2` (spaces per tab) |
| `Layout/IndentationWidth` | The first body line of a block is `Width` deeper than its opener line; closers align with the opener line. A tab counts as reaching the next multiple of `Width` columns; the fix re-indents with spaces. | convention | yes | `Width: 2` |
| `Layout/LeadingCommentSpace` | A space after `--`, `---` and `//` (block comments and dash-only lines exempt). | convention | yes | |
| `Layout/LineLength` | Lines are at most `Max` characters. The fix only changes whitespace and picks, per line, the first of these that fits: break after the `=` of an assignment or table field (value one level deeper); unfold the call argument list or table constructor holding the overrun (outermost first, a last table argument hugged as `f(a, {`): one item per line, the closer on its own line, lines of a trailing multi-line function re-indented; break a chain after its last `,`/`or`/`and`/`..` that fits (before the operator when the chain already leads with it; `if`/`elseif`/`while` conditions continue under the first condition), or before the `:` of a chained method call. A step that does not fit at once is taken when the following passes can finish it. Comment-only lines, overruns inside a trailing comment, aligned lines and lines without a break point stay uncorrected. | convention | yes | `Max: 120`, `AllowURI: true`, `IgnoreComments: false`, `IndentationWidth: 2` (indentation of inserted lines) |
| `Layout/SpaceAfterComma` | A space after every comma followed by code. | convention | yes | |
| `Layout/SpaceAfterNot` | No space between `!` and its operand. | convention | yes | |
| `Layout/SpaceAroundOperators` | One space around binary operators and `=`; none around `..`. Unary `-`, `#`, `!`, `not` are not binary. | convention | yes | `ConcatStyle: no_space` (or `space`), `AllowForAlignment: true` (extra spaces before an operator that lines up with the same operator on an adjacent line) |
| `Layout/SpaceBeforeAnonymousFunctionParen` | No space between `function` and the `(` of an anonymous function: `function(a)`; exactly one with `EnforcedStyle: space` (`function (a)`). | convention | yes | `EnforcedStyle: no_space` (or `space`) |
| `Layout/SpaceBeforeComma` | No space before a comma. | convention | yes | |
| `Layout/SpaceBeforeComment` | A space between code and a trailing comment. | convention | yes | |
| `Layout/SpaceBeforeParen` | No space between a function name and its `(`. | convention | yes | |
| `Layout/SpaceInsideBraces` | `{ a = 1 }` with one space inside non-empty braces, `{}` when empty. | convention | yes | `EnforcedStyle: space` (or `no_space`) |
| `Layout/SpaceInsideBrackets` | No spaces inside `[` `]` of indexing and table keys; exactly one with `EnforcedStyle: space`; with `space_when_nested`, exactly one inside a pair holding another pair (`a[ b[i] ]`) and none otherwise. | convention | yes | `EnforcedStyle: no_space` (or `space`, `space_when_nested`) |
| `Layout/SpaceInsideParens` | No spaces inside `(` `)`; with `EnforcedStyle: space`, exactly one inside non-empty parentheses and none in `()`. | convention | yes | `EnforcedStyle: no_space` (or `space`) |
| `Layout/TrailingEmptyLines` | Exactly one `\n` at the end of the file. | convention | yes | |
| `Layout/TrailingWhitespace` | No trailing spaces or tabs (outside strings). | convention | yes | |

### Style

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Style/AndOr` | `and`/`or` instead of `&&`/`\|\|`; with `EnforcedStyle: symbols`, `&&`/`\|\|` instead of `and`/`or`. | convention | yes | `EnforcedStyle: keywords` (or `symbols`) |
| `Style/CallParentheses` | Parentheses around the single string or table literal argument of a call: `f('x')`, `f({ 1 })`, `obj:m('x')`. The `omit` styles write `require 'x'` and `f { 1 }` (one space), and keep parentheses separated from their argument by a line break. Calls with comments in the way are not corrected. Disabled by default (Flux writes `require_relative 'x'` and `class 'X'` on purpose). | convention | yes | `EnforcedStyle: always` (or `omit`, `omit_for_strings`, `omit_for_tables`) |
| `Style/CommentStyle` | Comment markers: with `lua`, `//` → `--` (`///` → `---`) and `/* */` → `--[[ ]]` (with `=` levels when the body holds `]]`); with `c`, `--` → `//` and `--[[ ]]` → `/* */`, except doc comments (a run of comment-only lines containing a `---` line, kept for LDoc/EmmyLua and the documentation readers) and block comments containing `*/`. Lumen markup comments are never touched. | convention | yes | `EnforcedStyle: lua` (or `c`) |
| `Style/DocumentationMethod` | Every named function definition is directly preceded by a doc comment starting with `---`. | convention | no | `RequireForLocalFunctions: false` |
| `Style/IncompleteDocumentation` | A function whose doc comment has a parameter or return tag documents every named parameter (names starting with `_` exempt) and has an `@return` (or `@treturn`) when its own body returns a value other than `nil`. Local functions are skipped unless `RequireForLocalFunctions` is set. Disabled by default. | convention | no | `RequireForLocalFunctions: false`, `TypedParamTags: [string, number, int, bool, tab, func, thread]` |
| `Style/InequalityOperator` | `!=` instead of `~=`; with `EnforcedStyle: tilde`, `~=` instead of `!=`. | convention | yes | `EnforcedStyle: bang` (or `tilde`) |
| `Style/InvertIf` | An `if` with an `else` (no `elseif`) whose `else` is a single `return` with at most one value (or `break`/`continue` in a loop), whose `if` branch has at least `MinBodyLength` statements and does not end by leaving, followed by more code: invert it into a guard clause. Disabled by default. | convention | no | `MinBodyLength: 3` |
| `Style/Not` | `!` instead of `not`; with `EnforcedStyle: keyword`, `not` instead of `!` (`!!x` becomes `not not x`). | convention | yes | `EnforcedStyle: bang` (or `keyword`) |
| `Style/ParenthesesAroundCondition` | No parentheses wrapping the whole condition of `if`, `elseif`, `while` or `until`; with `EnforcedStyle: require` (vanilla GMod's `if ( x ) then`), every such condition is one parenthesised expression (`if (a) and (b) then` is wrapped as a whole). | convention | yes | `EnforcedStyle: omit` (or `require`) |
| `Style/PreferredLocalAlias` | Uses of a path such as `table.insert` after a file-level alias of it (`local insert = table.insert`; a path starting with a global, alias never reassigned) where the alias is not shadowed and the path's first name is still the global. Uses that are assigned to, chains not starting at the global (`x.table.insert`) and paths assigned anywhere in the file are exempt. The fix replaces the path with the alias. | convention | yes | |
| `Style/Semicolon` | No semicolons (a `;` between table fields becomes a comma; a `;` between statements of a function inside a table stays a statement separator and is removed). With `EnforcedStyle: require` (Clockwork), every statement ending a line ends with `;`, including the `end` of blocks and functions (`end;`, `end);`), while lines ending in `then`, `do`, `else` or a function header take none; stray semicolons (`then;`, `;;`) are removed. | convention | yes | `EnforcedStyle: omit` (or `require`) |
| `Style/StringLiterals` | Single quotes, unless the string contains a single quote; long strings exempt. | convention | yes | `EnforcedStyle: single_quotes` (or `double_quotes`) |
| `Style/TrailingCommaInTable` | No comma after the last item of a table. | convention | yes | `EnforcedStyle: no_comma` (or `comma`: required in multi-line tables) |

### Lint

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Lint/DuplicateRegistration` | The same literal network string (`util.AddNetworkString`), network receiver (`net.Receive`), console command (`concommand.Add`) or console variable (`CreateConVar`, `CreateClientConVar`) registered again, case-insensitively, after a registration in the same or an enclosing block (the `SERVER` and `CLIENT` branches of an `if` do not conflict). Files that do not parse are skipped. | warning | no | |
| `Lint/DuplicateRequire` | A call of a function in `Functions` with a string literal already passed to an earlier such call in the same or an enclosing block; calls whose result is indexed right away (`require('x').y`) are exempt. Files that do not parse are skipped. | warning | no | `Functions: [require]` |
| `Lint/DuplicateTableKey` | The same literal key twice in one table constructor (`a = 1, a = 2`, `['a']`, `[1]`/`[1.0]`). | warning | no | |
| `Lint/FormatArgumentCount` | A call of a function in `Functions` (`string.format(fmt, ...)`, `Format(fmt, ...)`) or the method form on a string literal (`('%d'):format(n)`) whose literal format takes more arguments than given (a "bad argument" error at run time) or fewer (the extra ones are ignored). Every `%[-+ #0]*[0-9]*(.[0-9]*)?[cdiouxXeEfgGqsaA]` conversion takes one argument and `%%` none; formats with any other `%` sequence or a numeric escape are skipped. When the last argument is a call or `...`, only the arguments before it count and only too many are flagged. Files that do not parse are skipped. | warning | no | `Functions: [string.format, Format]` |
| `Lint/GlobalAssignment` | Assignments to a plain name that resolves to no local (`count = 0`) and `function name()` statements with a single-name path, inside a function body: almost always a missing `local`. Exempt are file-level writes (unless `AllowFileLevel` is `false`) and in-function writes of a name also assigned at file level in the same file, names in `AllowedGlobals`, names without lowercase letters (`CW_CONVAR_LOG`) with `AllowUppercase`, and globals the function reads before or in the statement of its first write there (`local old = can`, `if !sfs then sfs = ... end`, `n = n + 1`). Each global is reported once per function (the innermost one), at its first write. Files that do not parse are skipped. | warning | no | `AllowFileLevel: true`, `AllowUppercase: true`, `AllowedGlobals: [GM, GAMEMODE, ENT, SWEP, EFFECT, TOOL, PANEL, PLUGIN, SCHEMA, Schema, ITEM, FACTION, CLASS, ATTRIBUTE, LANGUAGE, CHAT_CLASS, PACKAGE, CMD, ROLE, THEME, SKILL, PLAYER]` |
| `Lint/NonLiteralAssertMessage` | `assert(condition, message)` whose message is not a literal, a name, a field or an index, so it is built even when the assertion passes. Disabled by default. | warning | no | |
| `Lint/RedefinedLabel` | A `::label::` whose name another label of the same block already uses (a LuaJIT compile error). Function bodies start afresh. | error | no | `AllowNestedRedefinition: true` (when `false`, also flag reusing the name of a label in an enclosing block of the same function) |
| `Lint/RedefinedLocal` | Locals, `local function`s, parameters and loop variables whose name is already a visible local (same scope or any enclosing one, across functions), including `local x = x`. Names starting with `_` and `self` are exempt, and so is `local PANEL = {}` reused after the previous table was passed to `vgui.Register`, `vgui.RegisterTable` or `derma.DefineControl`; files that do not parse are skipped. | warning | no | `AllowParameters: false` (parameters may shadow outer locals, but not each other) |
| `Lint/RemoveInForwardLoop` | `table.remove(t, i)` inside a loop walking `t` forward with the index `i`, which skips the element after each removed one: numeric `for i = start, limit do` loops without a step or with a positive numeric literal step whose limit mentions `#t` (`for i = 1, #t do`), and `for i, v in ipairs(t) do` and `for k, v in pairs(t) do` loops. The removal must name the walked table (the same name or field chain of the same variable) and pass exactly the loop variable (a local shadowing it does not count); removals inside functions defined in the loop are ignored, and so is a removal followed in its block, or an enclosing block inside the loop, by `return`, `goto` or a `break` leaving the walked loop. Files that do not parse are skipped. | warning | no | |
| `Lint/SelfAssignment` | Assignments of a name, field or index to itself (`x = x`, `a.b = a.b`, `a[k] = a[k]`, `t['x'] = t.x`), including one target of a multiple assignment whose value in the same position is itself (`a, b = a, c` flags `a`; the swap `a, b = b, a` is fine). Field and index chains must start at a name and contain no calls, with literal or plain-name keys; `local x = x` (caching a global or upvalue) is not flagged. Files that do not parse are skipped. | warning | no | |
| `Lint/ShadowedLibrary` | Locals, parameters and loop variables named after a GMod library (`local file = ...`); `local render = render` is allowed. | warning | no | `Libraries: [player, team, file, table, sound, string, math, util, net, hook, timer, render, surface, draw, ents, game, engine, input, gui, vgui, http, os, debug, bit]` |
| `Lint/Syntax` | Unterminated strings and block comments, stray characters, the first unbalanced bracket, `end` or `until`, and the first piece of malformed Lumen markup. When none of those is found, the file is parsed and the first token the grammar rejects is reported (``Unexpected `y`; expected `then`.``): code after a `return` in the same block, an expression standing as a statement, assigning to a call, a missing `=` or `,`, and so on. | fatal | no | |
| `Lint/UnbalancedAssignment` | Assignments with more targets than values (`local a, b = 1`: `b` is `nil`), unless the last value is a call, `...` or `nil`, and values beyond the number of targets (`local a = 1, 2`), which are discarded. | warning | no | |
| `Lint/UncheckedEntityInCallback` | Method calls (`e:Method(...)`) on a captured entity inside a deferred callback, a function literal passed to one of the `Functions` (the number is the 1-based position of the callback argument; closures nested in it count), when the callback does not check `IsValid(e)` or `e:IsValid()` anywhere before the call (by source position): the entity may have been removed by then ("Tried to use a NULL entity!"). `e` is a local, parameter or loop variable declared outside the callback that holds an entity: `self` in a method or function field of one of the `EntityTables` (`function ENT:Think()`, `ENT.Use = function(self)`), a variable named one of the `EntityNames`, or a local initialized from a call of one of the `EntityFunctions`; a local initialized from one of the `AlwaysValidFunctions` never counts. Field accesses and arguments are not checked; each variable is reported once per callback, at its first unchecked call. Files that do not parse are skipped. | warning | no | `Functions: { timer.Simple: 2, timer.Create: 4 }`, `EntityTables: [ENT, SWEP, EFFECT, PANEL, TOOL]`, `EntityNames: [ply, player, pl, client, ent, entity, target, victim, attacker, inflictor, owner, weapon, wep, npc, vehicle, ragdoll, prop, panel]`, `EntityFunctions: [ents.Create, ents.CreateClientProp, Entity, player.GetBySteamID, player.GetBySteamID64, vgui.Create]`, `AlwaysValidFunctions: [LocalPlayer]` |
| `Lint/UndefinedDocParam` | Parameter tags in a function's doc comment (`@param name`, `@param name=default`, `---@param name?`, `@tparam type name`, `TypedParamTags` aliases such as `@string name`) that name no parameter of the function; methods have an implicit `self`, `...` matches a vararg. The doc comment is the `--` comment lines directly above the line of the `function` keyword, when it is the first function on its line. Files that do not parse are skipped. | warning | no | `TypedParamTags: [string, number, int, bool, tab, func, thread]` |
| `Lint/UnknownDocTag` | Tags (`@name` starting the text of a `--` line comment) that are not in `KnownTags`. Disabled by default. | warning | no | `KnownTags` (EmmyLua/LuaLS, LDoc, GLuaLS and Flux tags; see `--show-readers Lint/UnknownDocTag`) |
| `Lint/UnreachableCode` | Statements after one that always leaves the block: `break`, `continue`, `goto`, `return`, a call of a terminating function, a `do ... end` that always leaves, an `if` with an `else` whose every branch leaves. A `::label::` makes the code after it reachable again; each unreachable run is reported once. | warning | no | `TerminatingFunctions: [error]` |
| `Lint/UnusedSelf` | Methods (`function a:b()`) that never read their implicit `self` and could be defined with `.`. Disabled by default: GMod hooks and scripted classes are methods by convention. | warning | no | |
| `Lint/UnusedVariable` | Locals, `local function`s, parameters and loop variables that are never read, including ones only ever assigned to (`local x; x = 1`). A Lumen component tag (`<Row />`) reads the variable it names. Names starting with `_`, `self` and `...` are exempt; files that do not parse are skipped. The fix removes an unused local with every assignment to it: a statement binding only that name is deleted (with its line when alone on it, and a removed function's doc comment), one binding several names loses the name and its value. Only values without side effects are removed; a local whose declaration or assignments hold a call (`local margin = SScaleMin(20 / 3)`) is prefixed with `_` instead, and so are parameters and loop variables: at the declaration, at every assignment and in the matching `@param` tags of the function's doc comment, unless the prefixed name already appears in the variable's scope. | warning | yes | `AllowUnusedParameters: false` (when `true`, parameters are never flagged) |

### Performance

The `Performance` section holds the settings its readers share: `HotPaths`, the hooks and the entity, weapon
and panel methods that run every frame or tick (`Think`, `Tick`, `HUDPaint`, `Paint`, `Draw`, `CalcView`,
`Move`, the `PreDraw*`/`PostDraw*` hooks, ...; `--show-readers Performance` lists them), and
`HookFunctions: [hook.Add]`, the functions registering a handler as `f(event, identifier, handler)`. A hot
path is a function defined under one of the `HotPaths` names as a method or field (`function GM:Think()`,
`function PANEL:Paint(w, h)`, `ENT.Draw = function(self)`, `{ Think = function() end }`) or registered for
one of them through a `HookFunctions` call (`hook.Add('HUDPaint', 'id', handler)`; a handler naming a function
defined in the file counts); functions nested in a hot path are part of it.

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Performance/ConstantObjectInHotPath` | Calls of the constructors in `Functions` with only constant arguments (`Color(0, 0, 0, 200)`, `Vector(0, 0, 1)`; literals, parentheses and operators over literals) inside a hot path, which build the same object every frame or tick; a local outside the hot path could hold it. Calls without arguments are exempt. Files that do not parse are skipped. | warning | no | `Functions: [Color, Vector, Angle]` |
| `Performance/DistanceComparison` | A call of one of the `Methods` (`pos:Distance(other) < 100`) or `Functions` (`math.sqrt(x) <= y`) standing as an operand of a comparison, where the square root is wasted: the squared method (`DistToSqr`) or the function's argument compared with the squared bound gives the same answer. A distance inside a larger expression (`a:Distance(b) + c:Distance(d) < 100`) is left alone. The fix squares the call and the bound: a numeric literal becomes its square (`== 100` to `== 10000`), a name or field is multiplied by itself (`range * range`), anything else is raised to the second power (`self:GetRange() ^ 2`, `(range + 1) ^ 2`); two compared square roots are squared together, and a negated literal bound is not corrected. Files that do not parse are skipped. | warning | yes | `Methods: { Distance: DistToSqr, Distance2D: Distance2DSqr, Length: LengthSqr, Length2D: Length2DSqr }`, `Functions: [math.sqrt]` |
| `Performance/GetAllLoop` | Generic `for` loops over the table a function in `Iterators` builds (`for _, ply in ipairs(player.GetAll()) do`, also with `pairs(...)` or `next, player.GetAll()`), which the matching iterator walks from a cached table instead of building a new one on every call. Tables kept in a variable, or used other than as the loop's table, are left alone. The fix writes `for _, ply in player.Iterator() do`, unless a comment lies among the loop's iterator expressions. Files that do not parse are skipped. | warning | yes | `Iterators: { player.GetAll: player.Iterator, ents.GetAll: ents.Iterator }` |
| `Performance/SequentialPairs` | `pairs(t)` (or `next, t`) in a generic `for` loop when `t` is known to be a sequence: `ipairs` visits the same entries in order, and LuaJIT compiles it where it cannot compile `pairs`. A sequence is a table constructor of positional values that cannot be `nil` (literals, tables, functions, operators other than `and`), a call of one of the `SequentialFunctions`, or a local declared as one of those, never assigned again, and otherwise only read (`#t`, `t[k]`, `t.name`), passed to `pairs`, `ipairs`, `next` or one of the `SafeFunctions`, or grown and shrunk with `table.insert(t, v)`, `t[#t + 1] = v` and `table.remove` outside the loop. The fix writes `ipairs(t)`. Files that do not parse are skipped. | warning | yes | `SequentialFunctions: [player.GetAll, player.GetHumans, player.GetBots, team.GetPlayers, ents.GetAll, ents.FindByClass, ents.FindByModel, ents.FindByName, ents.FindInBox, ents.FindInSphere, ents.FindInCone, ents.FindInPVS, ents.FindAlongRay, string.Explode, string.Split, string.ToTable, table.GetKeys, table.ClearKeys, file.Find]`, `SafeFunctions: [unpack, table.unpack, table.concat, table.sort, table.Count, table.HasValue, table.KeyFromValue, table.Random, table.IsEmpty, table.Copy, table.GetKeys, table.maxn, istable, type, tostring, print, PrintTable]` |
| `Performance/DivisionByConstant` | Division by a numeric literal that is a power of two (`x / 2`, `x / -4`, `x / 0.5`), which multiplying by its reciprocal computes exactly and faster: the fix writes `x * 0.5`, `x * -0.25`, `x * 2`. Divisors without an exact reciprocal (`/ 3`, `/ 10`: `3 / 10` is `0.3` but `3 * 0.1` is not) are left alone, and so are divisors above `MaxDivisor` and fractional ones below its inverse, whose reciprocals get long (`/ 1024` would become `* 0.0009765625`), divisions of a constant (`255 / 256`, which Lua folds when it compiles the file), and hex divisors (`code / 0x40`, which read as bit shifts). LuaJIT already compiles such divisions as multiplications, so this saves time where code runs in the interpreter. A value whose metatable has `__div` but not `__mul` breaks under the fix. Files that do not parse are skipped. | warning | yes | `MaxDivisor: 256` |
| `Performance/TypeComparison` | `type(x) == 'number'` and `type(x) != 'number'` (either order, `~=` too) when `Predicates` maps the type name to a predicate that tests it directly, without building and comparing a string. The fix writes `isnumber(x)` or `not isnumber(x)`. | warning | yes | `Predicates: { number: isnumber, string: isstring, boolean: isbool, table: istable, function: isfunction, Vector: isvector, Angle: isangle, Panel: ispanel, VMatrix: ismatrix }` |
| `Performance/TableInsertAppend` | `table.insert(t, v)` standing as a statement with exactly two arguments, which `t[#t + 1] = v` does without calling a function. Only tables that can be written twice are flagged: a name, or fields and indexes over a name whose keys are literals or names (`t`, `self.items`, `lists[id]`). Calls with a `...` value, and calls where `table` is a local, are left alone. The fix writes `t[#t + 1] = v`, unless a comment lies inside the call or the value calls a function and mentions the table's root name (`table.insert(t, build(t))`, where the call could grow the table before `#t` is taken). A call as the value is assumed to return one value; unlike `table.insert`, the assignment goes through a `__newindex` metamethod. Files that do not parse are skipped. Disabled by default: `table.insert` is the default style. | warning | yes | |
| `Performance/PowerOfTwo` | Squaring with `^ 2` (`x ^ 2`, `self.radius ^ 2.0`), which calls `pow` where multiplying the value by itself is cheaper: the fix writes `x * x`. Only bases that can be written twice are flagged: a name, or fields and indexes over a name whose keys are literals or names. The product is parenthesized where it would otherwise bind looser than `^`: as the operand of a unary operator (`-x ^ 2` to `-(x * x)`) and as the right operand of `*`, `/`, `%` and `^` (`a / b ^ 2` to `a / (b * b)`). No fix when a comment lies inside the expression. LuaJIT already compiles `x ^ 2` as a multiplication in traces, so this saves time where code runs in the interpreter. A value whose metatable has `__pow` but not `__mul` breaks under the fix. Files that do not parse are skipped. | warning | yes | |
| `Performance/ConcatInLoop` | Strings built with `..` in a loop: an assignment `s = s .. x` (the target is the leftmost operand of the concatenation) or `s = x .. s` (the rightmost) in the body of a `for`, `while` or `repeat` loop, where `s` is a name or a field chain over a name (`self.text`) declared outside the loop. Each such assignment copies the whole string built so far; collecting the parts in a table and calling `table.concat` copies them once. Targets declared inside the loop (locals of its body, loop variables, fields of them) and loops in a function defined outside them are left alone. Each target is reported once per loop, at its first assignment, in the outermost loop of its function that it outlives. Files that do not parse are skipped. | warning | no | |
| `Performance/GlobalLookupInHotPath` | Library members looked up through their global (`surface.SetDrawColor`, `math.sin`) at least `MinUses` times in one hot path (with the functions nested in it). Each lookup indexes the global table and then the library on every frame or tick; a file-level `local SetDrawColor = surface.SetDrawColor` does it once (and `Style/PreferredLocalAlias` then keeps the code using it). Only the `Libraries` count, and only where the library name is the global rather than a local; assignments to a member are not lookups. Each path is reported once per hot path, at its first lookup. Disabled by default. Files that do not parse are skipped. | warning | no | `Libraries: [math, surface, draw, render, cam, string, table, util, input, gui]`, `MinUses: 2` |
| `Performance/ExpensiveCallInHotPath` | Calls of expensive functions inside a hot path. The expensive functions are `UnsafeFunctions` plus `ExtraUnsafeFunctions` and the keys of `Alternatives`, minus `SafeFunctions`; a call of a function with an `Alternatives` entry recommends that alternative (`Alternatives: { Material: util.get_material }`), any other suggests moving the call out of the hot path. Files that do not parse are skipped. | warning | no | `UnsafeFunctions: [player.GetAll, ents.GetAll, file.Append, file.CreateDir, file.Delete, file.Exists, file.Find, file.IsDir, file.Open, file.Read, file.Rename, file.Size, file.Time, file.Write, CompileString, RunString, RunStringEx, table.HasValue, surface.CreateFont, surface.GetTextureID, Material, vgui.Create]`, `ExtraUnsafeFunctions: []`, `SafeFunctions: []`, `Alternatives: {}` |

### Security

Server code that trusts what clients send. The readers about handlers skip code that only runs on the
client: files named `cl_*` (`cl_init.lua`) or in a `client` directory, the body of `if CLIENT then` (also
`if CLIENT and x`, `if not SERVER`), the `elseif`/`else` branches after `if SERVER then`, and the rest of a
block after a guard such as `if SERVER then return end`. Everything else (`sv_`, `sh_`, `init.lua`,
`shared.lua`, unguarded code) may run on the server. A handler is a function literal passed to the
registering call or a function of the file passed by name (`local function f`, `function a.b()`,
`local f = function`); functions nested in it count.

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Security/NetMessageExecution` | A call of one of the `Sinks` inside a `net.Receive` handler that may run on the server when an argument holds data read from the message: a call of one of the `Sources`, or a variable or field assigned anywhere in the handler from an expression holding such data (through `..`, `string.format` and other calls, table constructors, fields and loops over tainted tables). Comparisons, `not`, `#`, unary minus, `tonumber`, `tobool`, `type`, `IsValid` and the `is*` predicates do not pass the data on. Handlers registered with one of the `PayloadReceivers` (methods written `a.b:c`), called with the sending player followed by the message's values, are checked with every parameter after the first (and `...`) holding message data. Files that do not parse are skipped. | error | no | `Sources: [net.ReadString, net.ReadData, net.ReadTable, net.ReadType]` (the numeric, boolean and entity readers are left out), `Sinks: [RunString, RunStringEx, CompileString, game.ConsoleCommand, RunConsoleCommand, file.Write, file.Append, file.Delete, file.Rename]`, `PayloadReceivers: [netstream.Hook, cable.receive, Cable.receive]` |
| `Security/NetReadTable` | `net.ReadTable()` inside a `net.Receive` handler that may run on the server: a client can send an arbitrarily large or deeply nested table and stall the server; typed reads (`net.ReadUInt`, `net.ReadString`, ...) are safer. Files that do not parse are skipped. | warning | no | |
| `Security/SqlInjection` | Queries built from values that are not escaped: the first argument of a call of one of the `Functions` or of a method named in `Methods`, when it is a `..` concatenation or a `string.format`/`Format`/`fmt:format` call, directly or in a local assigned once (`local q = '...' .. name`). Every piece that is not safe is reported. Safe pieces are literals, calls of the `Escapers` or of a function of the file that only returns such values (`local function esc(s) return sql.SQLStr(s) end`), calls of methods named in `EscapeMethods`, arithmetic and `#`, `a and b or c` over safe values, arguments of a literal format taken by a numeric conversion (`%d`, `%f`), numeric `for` loop variables, and locals assigned once from a safe value. A literal query, or one that is just a name, parameter or call, is not flagged. Files that do not parse are skipped. | warning | no | `Functions: [sql.Query, sql.QueryRow, sql.QueryValue]`, `Methods: [query]` (mysqloo's `db:query`), `Escapers: [sql.SQLStr, SQLStr, tonumber, math.floor, math.Round]`, `EscapeMethods: [escape, quote, quote_name]` (mysqloo, gmsv_pg) |
| `Security/UncheckedConsoleCommand` | `concommand.Add(name, callback)` whose callback may run on the server, calls something privileged (one of the `PrivilegedFunctions`, or a method named in `PrivilegedMethods`) and never makes a permission check: a call, anywhere in the callback, of one of the `PermissionChecks` (a dotted entry such as `CAMI.PlayerHasAccess` matches that function, a plain entry such as `IsAdmin` a method by that name or a global function). Privileged calls that only run for the server console are exempt: inside `if not IsValid(ply) then` (`!ply:IsValid()`, `ply == NULL`) or after `if IsValid(ply) then return end`. Disabled by default. Files that do not parse are skipped. | warning | no | `PrivilegedFunctions: [RunConsoleCommand, game.ConsoleCommand, RunString, RunStringEx, CompileString, file.Write, file.Append, file.Delete, ents.Create, game.CleanUpMap]`, `PrivilegedMethods: [Kick, Ban, SetUserGroup, SetHealth, SetArmor, Give, StripWeapons, GodEnable]`, `PermissionChecks: [IsAdmin, IsSuperAdmin, IsUserGroup, CheckGroup, HasPermission, IsListenServerHost, query, CAMI.PlayerHasAccess, ULib.ucl.query]` |

### Naming

| Reader | Checks | Severity | Fix | Options (defaults) |
|--------|--------|----------|-----|--------------------|
| `Naming/ClassName` | The class and module names given to `class 'Name'`, `mod 'Name'` and `extends 'Name'` (the `Functions`) are PascalCase in every `::` or `.` segment (`'Lumen::Compiler'`; acronyms such as `'MVC'` allowed). | convention | no | `Functions: [class, mod, extends]` |
| `Naming/FieldName` | Fields the code defines (`self.max_size = 4`, `{ max_size = 4 }`) follow `EnforcedStyle`, judged like `Naming/VariableName`. Fields of tables named in capitals (`SKIN`, `ENT`, `TOOL`) and names matching `AllowedNames` (glob patterns) are exempt; functions defined with `function a.b()` are left to `Naming/MethodName`. | convention | no | `EnforcedStyle: snake_case` (or `camelCase`, `PascalCase`), `AllowedNames: ['m_*', 'col[A-Z]*', 'tex[A-Z]*', onSuccess, onError, onAborted, onData, onConnected, onConnectionFailed]` (GMod member fields, Derma skin colours and textures, MySQLOO callbacks) |
| `Naming/MethodName` | Defined functions and methods (including `local function`, last name segment) follow `EnforcedStyle`: `snake_case` flags lowerCamelCase, and PascalCase except where GMod or Lumen need it (methods such as hooks, functions of tables named in capitals like `TOOL.BuildCPanel`, functions of lowercase global libraries like `hook.Call`, global functions like `LerpColor`, Lumen components); `camelCase` flags snake_case; `PascalCase` flags names starting with a lowercase letter. SCREAMING_SNAKE_CASE suits every style; Lua metamethods (`__index`, ...) are never flagged. `LocalFunctionStyle` gives `local function` names their own style. | convention | no | `EnforcedStyle: snake_case` (or `camelCase`, `PascalCase`), `LocalFunctionStyle: same` (or one of the styles) |
| `Naming/VariableName` | Locals, parameters, loop variables and assigned globals follow `EnforcedStyle`: `snake_case` flags lowerCamelCase (SCREAMING_SNAKE_CASE and ConstantStyle allowed), `camelCase` flags snake_case, `PascalCase` flags names starting with a lowercase letter. Single lowercase words suit `snake_case` and `camelCase`. | convention | no | `EnforcedStyle: snake_case` (or `camelCase`, `PascalCase`) |

### Known setting conflict

`Layout/ExtraSpacing` with `ForceEqualSignAlignment: true` pads the `=` of consecutive
assignments into one column, while `Layout/SpaceAroundOperators` with `AllowForAlignment: false`
removes every extra space before an `=`. Enabling both makes the two readers undo each other on
every autocorrect pass (until the pass limit is reached); keep `AllowForAlignment: true` when
forcing alignment.

## Lumen templates

A `.lumen` file is GLua in which markup may appear wherever an expression may, as compiled by Flux's
`Lumen.Compiler`:

```lua
local Row = Lumen.require('row')

return function(props)
  return <view style={{ gap = 4 }}>
    <text color="accent">Hello, {props.name}!</text>
    {Lumen.map(props.items, function(item)
      return <Row key={item.id} item={item} />
    end)}
  </view>
end
```

A `<` starts markup when it is followed by a letter, `_` or `>` and the token before it cannot end an operand
(`a < b` and `#list < 3` stay comparisons). Tags, attributes and their quoted values, text children and
`<!-- -->` comments are lexed as markup tokens; the Lua inside `{ }` splices is lexed as Lua and checked like
any other Lua, markup nested in it included. Autocorrection never changes a byte of markup, only the Lua
around and inside it and the whitespace between markup lines (indentation, trailing whitespace, blank lines).

How the readers treat markup:

- Markup is an operand to every reader. Operators, quotes, `=` and braces inside markup are not Lua tokens, so
  the spacing readers, `Style/StringLiterals`, `Style/TrailingCommaInTable`, `Style/Semicolon` and
  `Lint/DuplicateTableKey` only see the Lua in it. `Layout/ExtraSpacing` leaves the whitespace just inside the
  braces of a splice (`{ name }`) alone.
- Lines that start inside a markup element (all its lines but the first) are skipped by the indentation
  readers (`Layout/IndentationWidth`, `Layout/IndentationConsistency`), the blank-line readers
  (`Layout/EmptyLineAfterBlock`, `Layout/EmptyLineAfterGuardClause`, `Layout/EmptyLineBeforeBlock`,
  `Layout/EmptyLineBetweenDefs`, `Layout/EmptyLinesAroundBlockBody`) and equal-sign alignment, including the
  Lua spliced into them. The Lua lines around markup are checked as usual, and re-indenting a statement or
  block body moves the markup lines it holds along with it.
- `Layout/LineLength` reports long lines holding markup but does not break them.
- `Layout/TrailingWhitespace`, `Layout/IndentationStyle`, `Layout/EmptyLines`, `Layout/TrailingEmptyLines`
  and `Layout/EndOfLine` treat quoted attribute values and `<!-- -->` comments that span lines like
  multi-line strings: their contents are never changed.
- `Lint/Syntax` reports the first piece of malformed markup in the compiler's words (`'<view>' is never
  closed.`, `'</view>' expected, got '</text'.`).
- `Lint/UnusedVariable` counts a component tag as a read of the variable it names: `Row` for `<Row />`,
  `Flux` for `<Flux.Panels.Card/>`. Only tags starting with an upper case letter are components; `<view>`
  names no variable.

## Flux conventions

The defaults encode the style of the Flux framework:

- 2-space indentation, no tabs, no trailing whitespace, LF line endings, exactly one newline at
  the end of the file, lines of at most 120 characters.
- Single-quoted strings unless the string contains `'`; `!` and `!=` instead of `not` and `~=`;
  `and`/`or` instead of `&&`/`||`.
- `'a'..b..'c'`: no spaces around `..`, one space around every other binary operator and `=`.
- `{ a = 1, 'x' }` with spaces inside non-empty braces and `{}` when empty; no spaces inside
  parentheses or brackets; one space after a comma and none before; no space before a call's `(`.
- PascalCase class and module names (`class 'DateTime'`, `mod 'Lumen::Compiler'`); snake_case
  variables, functions, methods and fields; no camelCase anywhere. PascalCase functions only where
  GMod or Lumen need them: hooks, overrides of GMod library functions, GMod-style globals and Lumen
  components.
- Every function definition has a doc comment (`--- Summary.` followed by `-- @param` and
  `-- @return` lines).
- A blank line before every multi-line block statement and after its `end` (unless next to
  another opener or closer), after a one-line guard clause (`if !x then return end`) and between
  function definitions; no blank line right after an opener or right before `end`; never two
  blank lines in a row.
- No parentheses around conditions, no trailing commas in tables, no semicolons.
- Never name a variable after a GMod library (`player`, `file`, `table`, ...): use `actor`,
  `target`, `client`, `path`, `tbl` and the like.
- Nothing expensive in a function that runs every frame or tick (`Think`, `HUDPaint`,
  `PANEL:Paint`, ...): file access, `Material`, `surface.CreateFont`, `vgui.Create` and the like
  run once outside it, and a `Color`, `Vector` or `Angle` built from constants lives in a local
  outside it. Flux's `util.get_material` caches materials, so the Flux style recommends it over
  `Material` there. A distance or length that is only compared uses the squared form
  (`pos:DistToSqr(other) < range * range`) and skips the square root; a type test uses the
  predicate (`isnumber(x)`, `istable(x)`) rather than comparing `type(x)` with a string.
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
| `readers::layout::block_structure` | `BlockStructure`: per-line block nesting, line kinds (statement, continuation, closer, comment, blank, inside a multi-line token or markup), opener/closer lines, guard clauses, definitions and doc comments, plus re-indent and blank-line insertion edits. Used by the indentation and blank-line readers. |
| `readers::layout::spacing` | The gap between adjacent code tokens, unary/binary operator classification and the gaps inside bracket pairs. Used by the token-spacing readers. |
| `readers::layout::line_breaking` | `LineBreaker`: plans the whitespace-only line breaks of `Layout/LineLength`'s fix, simulating follow-up passes to check that a line can be finished. |
| `readers::layout::alignment` | Character columns and the alignment checks behind `AllowForAlignment` and `ForceEqualSignAlignment`. |
| `readers::naming::declarations` | Local variables, parameters, loop variables and named function definitions of a file. |
| `readers::naming::case` | `Case`, which classifies a name (one lowercase word, snake_case, lowerCamelCase, PascalCase, SCREAMING_SNAKE_CASE), the `NamingStyle` of the naming readers' `EnforcedStyle`, and `is_lower_camel_case`. |
| `readers::lint::nesting` | A bracket and block-keyword tracker used by `Lint/Syntax` and `Lint/DuplicateTableKey`. |
| `readers::lint::ast` | `parse(source)`: a syntax tree of statements and expressions (with byte ranges and tokens) built by a recursive descent parser that knows the GLua extensions and Lumen markup, a `Visitor` with `walk`, and helpers such as `Expression::dotted_name`, `as_call` and `string_contents`; a file that does not parse yields a `ParseError` at the first rejected token. Used by `Lint/Syntax` and the readers that need statement structure. |
| `readers::lint::scopes` | `resolve(source)`: a scope resolver that parses the token stream and returns every declared variable with its reads (Lumen component tags included), writes, scope and the variable it shadows, every name reference bound to its variable, and lookups by offset (`reference_at`, `visible_at`), or `None` when the file does not parse. Used by `Lint/UnusedVariable`, `Lint/RedefinedLocal`, `Lint/UnusedSelf` and `Style/PreferredLocalAlias`. |
| `readers::lint::libraries` | `GMOD_LIBRARIES`, the default list of `Lint/ShadowedLibrary`. |
| `readers::security::realm` | `is_client_file`, `client_ranges` (the code `if CLIENT then`, the `else` of `if SERVER then` and guards such as `if SERVER then return end` keep on the client) and `server_handlers(block, source, registrar)`: the functions a call such as `net.Receive` or `concommand.Add` registers where they may run on the server, as literals or functions of the file passed by name. Used by the `Security` readers. |
| `readers::performance::hot_paths` | `hot_calls(block, source, hot_paths, hook_functions)` and `configured_hot_calls(ctx, block)` (under the `HotPaths` and `HookFunctions` options of the `Performance` section, whose defaults it declares): every call of a plain function inside a hot path of a parsed file, with the name of the hot path it is in; plus `is_constant`. Used by the hot-path readers. |
| `readers::style::doc_comments` | The doc comment of every function of a parsed file (the `--` comment lines directly above its `function` keyword's line), its tags (`-- @param`, `---@param`) and documented parameter names with byte ranges, plus `comment_line`. Used by the documentation readers and `Lint/UnusedVariable`. |
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

- `Reader::default_severity` is `Warning` for `Lint/*`, `Performance/*` and `Security/*`, `Convention` otherwise; override it when needed.
- `Reader::default_enabled` is `true`; return `false` for a reader that is too noisy for most code bases, so
  that it only runs with `Enabled: true` (or when named in `--only`). `--show-readers` marks such readers
  `(disabled)`.
- `Reader::default_options` lists the reader's options with their defaults (`Yaml::Integer`, `Yaml::Boolean`,
  `Yaml::String`, `Yaml::Array`, `Yaml::Hash`); read them with `ctx.option_usize("Max", 120)`,
  `ctx.option_str_map("Alternatives")` and friends. Options shared by every reader of a department are
  declared once with `inventory::submit! { SharedOptions { department: "Performance", defaults: || ... } }`
  and read the same way: a reader sees its department's section under its own options.
- `ctx.source` gives the text, the lines and every token (trivia included) with byte ranges and 1-based
  line/column positions, plus helpers such as `code_tokens`, `token_at`, `first_code_token_on_line`,
  `lines_inside_multiline_tokens`, `lines_inside_markup`, `opaque_lines` and `component_references`. In a
  Lumen template (`source.is_lumen()`) markup is `TokenKind::Markup`; see [Lumen templates](#lumen-templates).
- A fix is a list of `Edit`s (`replace`, `remove`, `insert`) over byte ranges of the current text. Fixes that
  overlap an already accepted fix are retried in the next pass, so keep each fix minimal.
- Test with `crate::testing`: `inspect`, `inspect_with` (options as YAML, e.g. `"Max: 80"`), `inspect_configured`
  (a whole configuration file, for department sections), `autocorrect`,
  `expect_no_offenses`, `expect_offense`, `expect_offenses` and `expect_correction` (which also checks that the
  correction is idempotent). The `_lumen` variants (`inspect_lumen`, `expect_offense_lumen`, ...) inspect the
  snippet as a Lumen template.
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
                                             # that Layout/LineLength alone only breaks lines, shortens
                                             # them and adds no offenses of other readers, that the readers
                                             # rewriting code (Style/PreferredLocalAlias and the Performance
                                             # comparison fixes) change only what they are for; also
                                             # that correcting its Lumen templates keeps their markup
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Example styles

[`examples/.proofreader.yml`](examples/.proofreader.yml) lists every setting with its built-in
default (it is generated: `PROOFREADER_BLESS=1 cargo test --test cli` rewrites it after reader
changes). The folders of `examples/` hold complete styles, each a `.proofreader.yml` plus the same
example plugin written in that style; copy a configuration to the root of a project, or point
`inherit_from` at it.

| Style | Configuration | What it looks like |
|-------|---------------|--------------------|
| Flux | [`examples/flux`](examples/flux) | The built-in default: 2 spaces, `'single quotes'`, `!`, `!=`, `and`/`or`, `'a'..b`, `{ a = 1 }`, `function(a)`, PascalCase classes and modules, snake_case everything else (no camelCase), doc comments, blank lines around blocks, `util.get_material` recommended over `Material` in hot paths. |
| Catwork | [`examples/catwork`](examples/catwork) | The Flux style with Clockwork naming: lowerCamelCase variables, PascalCase functions, no required doc comments. |
| Clockwork | [`examples/clockwork`](examples/clockwork) | Tabs (4 columns), 120-character lines, PascalCase functions, lowerCamelCase variables, `{a, b, c}`, and `;` ending every statement that ends a line, `end;` included; otherwise the Flux style. |
| Vanilla GMod | [`examples/gmod`](examples/gmod) | The Lua shipped with the game: tabs, `print( "x" )`, `function( a )`, `t[ k ]`, `if ( !IsValid( ply ) ) then`, `"a" .. b`, lowerCamelCase variables, PascalCase functions, no line limit or blank-line rules. |
| Helix | [`examples/helix`](examples/helix) | Helix schemas and plugins: tabs, `if (condition) then`, `func(a, b)`, `function(a)`, `{a = 1}`, `array[ other[index] ]`, `"a"..b`, `"double quotes"`, `and`/`or`, `!`, lowerCamelCase locals and local functions, UpperCamelCase functions, unused parameters allowed, no line limit or blank-line rules. |
| C++ (Stroustrup) | [`examples/stroustrup`](examples/stroustrup) | 4 spaces, 80 columns, `//` and `/* */` comments, `&&`, `\|\|`, `!`, `!=`, `if (x > 0) then`, `"double quotes"`, `{1, 2}`, `f("x")`, snake_case. |

`cargo test --test examples` checks that every example is clean under its own style and that
autocorrecting the Flux example with each configuration produces that style's example
(`PROOFREADER_BLESS=1` regenerates them).
