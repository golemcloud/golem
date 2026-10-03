# Filesystem tools

A single Golem WASM component exporting `read-file`, `write-file`, `edit-file`, `ls`, and `grep`.
Paths may be workspace-relative or absolute, but empty paths, NUL bytes, and parent (`..`)
components are rejected. The WASI sandbox remains the confinement boundary.

`read-file`, `write-file`, and `edit-file` operate only on caller-supplied, known paths. `ls` and
`grep` deliberately discover entries below a caller-supplied root, using descriptor-relative WASI
operations and never following symbolic links.

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

`ls(path, max-depth, glob, limit, cursor)` requires a directory root and returns structured entries
in UTF-8-byte-sorted depth-first preorder. The root is depth zero and is not returned; the default
depth is one. A glob uses `globset` syntax and matches slash-separated paths relative to the root.
The default page limit is 100 and the maximum is 500. A continuation embeds the effective query,
must be passed back with identical arguments, and is valid only while the traversed filesystem
state remains compatible. Directory enumeration, traversal work, diagnostics, cursor size, and
returned strings have fixed safety limits. Oversized or inaccessible descendants are diagnostics;
invalid roots, options, globs, and cursors are typed errors.

`grep(path, pattern, mode, case-insensitive, max-depth, include-globs, exclude-globs, limit,
cursor)` searches either one regular file or a directory tree. Matching defaults to literal and
case-sensitive; regex mode uses Rust `regex` syntax. Matching is per complete logical line, returns
at most one result per matching line, strips LF or CRLF terminators, and never matches partial or
cross-line text. Paths are filtered relative to the root; exclusions win and matching excluded
directories are pruned. Files are limited to 1 MiB, lines to 64 KiB, returned line text to 4 KiB,
and every invocation is additionally bounded by files, bytes, lines, traversal entries,
diagnostics, output bytes, and the requested result limit. Binary, oversized, inaccessible, and
unsupported descendants produce structured diagnostics. Continuation hashes the active file and
rejects changed content.

Both discovery tools validate and reopen their explicit root on every invocation, including
continuations. An argument-level middleware such as PathPolicy authorizes the supplied root, not
each dynamically discovered descendant. It must authorize the entire requested traversal scope or
reject/narrow the request. Globs are filters, not authorization controls; WASI remains the
confinement boundary.

Build and copy the production artifact with the repository's Golem CLI:

```sh
golem build -P release --force-build --yes
golem exec -P release copy
```
