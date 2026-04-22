## Code Navigation with `moon ide`

**ALWAYS use `moon ide` for code navigation in MoonBit projects instead of manual file searching, grep, or semantic search.**

`moon ide` is the canonical navigation/refactoring entrypoint in current MoonBit toolchains.

## Core Commands

- `moon ide doc <query>`: Search API docs and symbols.
- `moon ide peek-def <symbol> [--loc path[:line[:col]]]`: Show definition context for a symbol.
- `moon ide find-references <symbol> [--loc path[:line[:col]]]`: Find usages of a symbol.
- `moon ide rename <symbol> <new_name> [--loc path[:line[:col]]]`: Rename a symbol semantically.
- `moon ide hover <symbol> --loc path:line[:col]`: Show type/doc info at a location.
- `moon ide outline <dir|file>`: List top-level symbols in a package/file.
- `moon ide analyze [path]`: Show public API usage summary of a package/module.

## Practical Examples

```bash
# Discover APIs in standard library or project packages
moon ide doc "String::*rev*"
moon ide doc "@buffer"

# Peek definition and references
moon ide peek-def Parser::read_u32_leb128
moon ide find-references TranslationUnit

# Resolve ambiguous symbol by location
moon ide peek-def parse --loc src/parser.mbt:42:8
moon ide rename parse parse_expr --loc src/parser.mbt:42:8

# Type/doc hover and package outline
moon ide hover filter --loc hover.mbt:14
moon ide outline .

# Public API usage analysis
moon ide analyze .
```

## Notes

- `moon ide goto-definition` and `-query`/`-tags` workflows are legacy and should not be used in new guidance.
- If syntax or flags are unclear, run `moon ide <command> --help`.
