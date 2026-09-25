# Rust filesystem tools

A single Golem WASM component exporting `read-file`, `write-file`, and `edit-file`.
Paths may be workspace-relative or absolute, but empty paths, NUL bytes, and parent (`..`)
components are rejected. The WASI sandbox remains the confinement boundary.

`read-file` accepts its optional inclusive line range as `[]` for the whole file, `[start]` from
one 1-based line through EOF, or `[start, end]` for a bounded read.

Build and copy the production artifact with the repository's Golem CLI:

```sh
golem build -P release --force-build --yes
golem exec -P release copy
```
