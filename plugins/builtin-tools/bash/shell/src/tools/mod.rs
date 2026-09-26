//! Embedded local command implementations.
pub mod coreutils;
pub(crate) mod devices;
pub(crate) mod diff;
pub(crate) mod env_options;
pub mod find;
pub(crate) mod grep;
pub mod install;
pub(crate) mod jq;
pub(crate) mod jq_syntax;
pub mod man;
pub(crate) mod patch;
pub(crate) mod programs;
pub(crate) mod sed;
pub(crate) mod sh;
pub(crate) mod shell_bytes;
pub mod stat;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod streaming;
pub mod texttools;
pub(crate) mod timeout;
pub mod which;
pub mod xargs;

/// The most any one buffer holds in memory: a call's stdout or stderr, a substitution, a finite
/// command's piped input or output, a synchronous builtin's output into a pipe. One limit for
/// everything bash-tool buffers, Brush's included.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "only wasm32 buffers a call's output and piped streams"
    )
)]
pub(crate) const MAX_BUFFER_BYTES: usize = brush_core::openfiles::MAX_SUBSTITUTION_BYTES;

/// [`MAX_BUFFER_BYTES`] as messages name it: `64 MiB`.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "only wasm32 buffers a call's output and piped streams"
    )
)]
pub(crate) fn buffer_limit() -> String {
    format!("{} MiB", MAX_BUFFER_BYTES >> 20)
}

/// A unique scratch path for a unit test. WASI has no `temp_dir()` or process id; the WASM test
/// runner preopens `/tmp`.
#[cfg(test)]
pub(crate) fn test_scratch(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(target_arch = "wasm32")]
    let (directory, process) = (std::path::PathBuf::from("/tmp"), 0);
    #[cfg(not(target_arch = "wasm32"))]
    let (directory, process) = (std::env::temp_dir(), std::process::id());
    directory.join(format!("{name}-{process}-{next}"))
}

/// An I/O error as C's `strerror` would word it: Rust's ` (os error N)` suffix stripped, so
/// messages match the GNU tools'.
pub(crate) fn io_message(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(index) => text[..index].to_owned(),
        None => text,
    }
}

/// Read a file operand. `/dev/null` reads as empty even where the filesystem has no `/dev`.
pub(crate) fn read_file(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    if path == std::path::Path::new("/dev/null") {
        Ok(Vec::new())
    } else {
        std::fs::read(path)
    }
}

/// Open a file operand, with `/dev/null` as an empty file.
pub(crate) fn open_file(path: &std::path::Path) -> std::io::Result<Box<dyn std::io::Read>> {
    if path == std::path::Path::new("/dev/null") {
        Ok(Box::new(std::io::empty()))
    } else {
        std::fs::File::open(path).map(|file| Box::new(file) as Box<dyn std::io::Read>)
    }
}
