// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

use crate::wasmtime_config::create_wasmtime_config_without_fs_cache;
use anyhow::Context;
use camino::Utf8Path;
use std::io::ErrorKind;
use std::path::Path;
use std::time::SystemTime;
use wasm_rquickjs::OptimizeResult;

pub const JS_COMPILATION_CACHE_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Preinitializes the current JavaScript source in an unchanged guest template.
/// Only compiled native code is cached; each call initializes a fresh instance.
pub async fn preinitialize_js(
    input: &Utf8Path,
    output: &Utf8Path,
    source: String,
    cache_dir: Option<&Path>,
) -> anyhow::Result<OptimizeResult> {
    let engine_config = create_wasmtime_config_without_fs_cache();
    let cache_dir = cache_dir.map(std::path::absolute).transpose()?;
    let cache_dir = cache_dir.as_deref();
    let cache_config = cache_dir.map(|directory| {
        let mut config = wasmtime::CacheConfig::new();
        config
            .with_directory(directory)
            .with_files_total_size_soft_limit(JS_COMPILATION_CACHE_MAX_BYTES);
        config
    });
    let sources = [source];
    let result = wasm_rquickjs::optimize_component_with_options(
        input,
        output,
        "wizer-initialize",
        &wasm_rquickjs::OptimizeOptions {
            hostcall_fuel: Some(512 * 1024 * 1024),
            sources: Some(&sources),
            engine_config: Some(&engine_config),
            cache_config: cache_config.as_ref(),
        },
    )
    .await;
    if let Some(directory) = cache_dir {
        let directory = directory.to_path_buf();
        let cleanup = tokio::task::spawn_blocking(move || trim_compilation_cache(&directory)).await;
        if let Err(error) = cleanup
            .context("Compilation cache cleanup task failed")
            .and_then(|r| r)
        {
            tracing::warn!(error = %error, "Failed to trim JavaScript compilation cache");
        }
    }
    result
}

fn trim_compilation_cache(directory: &Path) -> anyhow::Result<()> {
    // Wasmtime's background eviction has only a soft limit. Trim synchronously
    // after builds as well; concurrent compilation can temporarily exceed it.
    // Lock only maintenance, not compilation, so different apps build in parallel.
    let lock = std::fs::File::create(directory.with_extension("lock"))?;
    lock.lock()?;
    let mut files = Vec::new();
    let mut total = 0;
    for entry in walkdir::WalkDir::new(directory) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        total += metadata.len();
        files.push((
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata.len(),
            entry.into_path(),
        ));
    }
    if total > JS_COMPILATION_CACHE_MAX_BYTES {
        files.sort_by_key(|a| a.0);
        // Leave room for cache-worker metadata and concurrent publication.
        let target = JS_COMPILATION_CACHE_MAX_BYTES * 4 / 5;
        for (_, size, path) in files {
            match std::fs::remove_file(path) {
                Ok(()) => (),
                Err(error) if error.kind() == ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
            total -= size;
            if total <= target {
                break;
            }
        }
    }
    Ok(())
}
