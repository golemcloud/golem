WASI filesystem is a filesystem API primarily intended to let users run WASI
programs that access their files on their existing filesystems, without
significant overhead.

Paths are passed as interface-type `string`s, meaning they must consist of
a sequence of Unicode Scalar Values (USVs). Some filesystems may contain
paths which are not accessible by this API.

The directory separator in WASI is always the forward-slash (`/`).

All paths in WASI are relative paths, and are interpreted relative to a
`descriptor` referring to a base directory. If a `path` argument to any WASI
function starts with `/`, or if any step of resolving a `path`, including
`..` and symbolic link steps, reaches a directory outside of the base
directory, or reaches a symlink to an absolute or rooted path in the
underlying filesystem, the function fails with `error-code::not-permitted`.

For more information about WASI path resolution and sandboxing, see
[WASI filesystem path resolution].

Though this package presents a portable interface modelled on POSIX, it
prioritizes compatibility over portability: allowing users to access their
files on their machine is more important than exposing a single semantics
across all platforms.  Notably, depending on the underlying operating system
and file system:
  * Paths may be case-folded or not.
  * Deleting (unlinking) a file may fail if there are other file descriptors
    open.
  * Durability and atomicity of changes to underlying files when there are
    concurrent writers.

Users that need well-defined, portable semantics should use a key-value
store or a database instead.

[WASI filesystem path resolution]: https://github.com/WebAssembly/wasi-filesystem/blob/main/path-resolution.md