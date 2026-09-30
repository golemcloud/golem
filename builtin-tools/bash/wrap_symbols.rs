// The libc path entry points `tools::devices` answers `/dev` paths for on WASI, via `--wrap`.
// Included (not a crate: build scripts can't depend on one from the same workspace without a
// cycle) by both `shell/build.rs` and `component/build.rs`, so the two lists can't drift apart —
// a symbol added to one and not the other used to silently miss either the shipped component or
// the test harness, whichever build script wasn't updated.
const WRAPPED_PATH_SYMBOLS: &[&str] = &[
    "open",
    "openat",
    "stat",
    "lstat",
    "fstatat",
    "access",
    "faccessat",
    "readlink",
    "readlinkat",
    "read",
    "readv",
    "write",
    "writev",
    "close",
    "lseek",
    "fstat",
    "isatty",
    "ftruncate",
    "utimensat",
    "futimens",
    "unlink",
    "unlinkat",
    "rmdir",
    "mkdir",
    "linkat",
    "rename",
    "symlink",
    "opendir",
];
