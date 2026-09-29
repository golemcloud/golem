# Filesystem tools

A single Golem WASM component exporting `read-file`, `write-file`, and `edit-file`.
Paths may be workspace-relative or absolute, but empty paths, NUL bytes, and parent (`..`)
components are rejected. The WASI sandbox remains the confinement boundary.

These tools operate only on caller-supplied, known paths. They do not list, search, or discover
files; filesystem discovery is a separate capability.

`read-file(path, start-line, end-line, cursor)` reads UTF-8 text using optional, named 1-based line
bounds (`end-line` is inclusive). Each call traverses at most 64 KiB and 200 lines. Its result
contains the content and represented line bounds plus an optional continuation cursor. Pass the
cursor back unchanged with the same requested bounds until it is absent. Empty pages are expected
while advancing to a distant `start-line`, and an oversized line can span pages. UTF-8 characters
and CRLF sequences are not split between pages. Invalid ranges/cursors, missing or non-file paths,
binary data, and filesystem failures are reported as typed errors.

`write-file(path, content, create-parent-directories)` creates or replaces a known file with UTF-8
content. It reports whether the file was created or replaced and the byte count. Parent directories
are created only when explicitly requested. Unsafe paths, non-file destinations, and filesystem
failures are errors.

`edit-file(path, old-text, new-text)` replaces exactly one occurrence in a known UTF-8 file and
reports the replacement count and before/after byte sizes. Empty, missing, or multiply occurring
old text is rejected without editing; missing, non-file, binary, unsafe, and failed filesystem
operations are typed errors.

Build and copy the production artifact with the repository's Golem CLI:

```sh
golem build -P release --force-build --yes
golem exec -P release copy
```
