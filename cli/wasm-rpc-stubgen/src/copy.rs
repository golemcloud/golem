use anyhow::{anyhow, Context};
use std::path::Path;
use std::time::SystemTime;

// Differences compared to std::fs::copy
//  - ensures that the target dir exists
//  - updated the modtime after copy, which is not guaranteed to happen, making it not usable for
//    modtime based up-to-date checks (see https://github.com/rust-lang/rust/issues/115982 for more info)
//  - uses anyhow error with added context
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<u64> {
    let from = from.as_ref();
    let to = to.as_ref();

    let context = || {
        format!(
            "Failed to copy from {} to {}",
            from.to_string_lossy(),
            to.to_string_lossy()
        )
    };

    let target_parent = to
        .parent()
        .ok_or_else(|| anyhow!("Failed to get target parent dir"))
        .with_context(context)?;

    std::fs::create_dir_all(target_parent)
        .with_context(|| anyhow!("Failed to create target dir"))
        .with_context(context)?;

    let bytes = std::fs::copy(from, to).with_context(context)?;

    std::fs::File::open(to)
        .and_then(|to| to.set_modified(SystemTime::now()))
        .with_context(|| anyhow!("Failed to update target modification time"))
        .with_context(context)?;

    Ok(bytes)
}
