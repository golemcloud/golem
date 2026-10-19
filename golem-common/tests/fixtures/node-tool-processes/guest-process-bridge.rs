// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

use crate::bindings::golem::core::types::TypedSchemaValue;
use crate::bindings::golem::tool::{host, streams};
use crate::wrappers::{JsVec, UInt8Array};
use rquickjs::{Ctx, Exception};

type Output = wit_bindgen_p3::rt::async_support::StreamReader<host::ByteStreamItem>;

struct Stats {
    bytes: usize,
    checksum: u64,
    overflow: bool,
    failed: bool,
}

async fn drain(reader: Option<Output>, future: &host::FutureInvokeResult, max: usize) -> Stats {
    let mut stats = Stats {
        bytes: 0,
        checksum: 0,
        overflow: false,
        failed: false,
    };
    let Some(mut reader) = reader else {
        return stats;
    };
    while let Some(item) = reader.next().await {
        match item {
            Ok(chunk) => {
                let retained = chunk.len().min(max - stats.bytes);
                stats.bytes += retained;
                stats.checksum += chunk[..retained]
                    .iter()
                    .map(|byte| u64::from(*byte))
                    .sum::<u64>();
                if retained < chunk.len() {
                    stats.overflow = true;
                    future.cancel();
                    break;
                }
            }
            Err(streams::ByteStreamFailure::Cancelled) => break,
            Err(_) => {
                stats.failed = true;
                break;
            }
        }
    }
    stats
}

#[rquickjs::function]
fn collect_sync<'js>(
    ctx: Ctx<'js>,
    name: String,
    path: JsVec<String>,
    input: TypedSchemaValue,
    bytes: UInt8Array,
    takes_stdin: bool,
    has_stderr: bool,
    max: u32,
) -> rquickjs::Result<String> {
    let rpc = host::ToolRpc::new(&name);
    let (writer, stdin) = if takes_stdin {
        let (writer, source, watcher) = host::create_stdin();
        drop(watcher);
        (Some(writer), Some(source))
    } else {
        (None, None)
    };
    let (stdout_target, stdout) = host::create_output();
    let (stderr_target, stderr) = if has_stderr {
        let (target, stream) = host::create_output();
        (Some(target), Some(stream))
    } else {
        (None, None)
    };
    let future =
        rpc.async_invoke_and_await(&path.0, input, stdin, Some(stdout_target), stderr_target);
    let result = wasip3::wit_bindgen::rt::async_support::block_on(async move {
        let pump = async {
            if let Some(writer) = writer {
                for chunk in bytes.0.chunks(32 * 1024) {
                    writer
                        .write(chunk.to_vec())
                        .await
                        .map_err(|_| "stdin write failed")?;
                }
                writer.finish().await.map_err(|_| "stdin finish failed")?;
            }
            Ok::<_, &str>(())
        };
        let (terminal, pumped, out, err) = futures::join!(
            future.get(),
            pump,
            drain(Some(stdout), &future, max as usize),
            drain(stderr, &future, max as usize),
        );
        if out.overflow || err.overflow {
            return Ok(format!("overflow:{}", out.bytes));
        }
        let terminal = terminal.map_err(|_| "tool invocation failed")?;
        if terminal.result.is_none() || terminal.stdout.is_some() || terminal.stderr.is_some() {
            return Err("invalid structured tool terminal");
        }
        pumped?;
        if out.failed || err.failed {
            return Err("tool output failed");
        }
        Ok(format!(
            "{}:{}:{}:{}",
            out.bytes, out.checksum, err.bytes, err.checksum
        ))
    });
    result.map_err(|message| Exception::throw_message(&ctx, message))
}

pub struct ToolProcessBridge;
impl rquickjs::module::ModuleDef for ToolProcessBridge {
    fn declare(decl: &rquickjs::module::Declarations) -> rquickjs::Result<()> {
        decl.declare("collectSync")?;
        Ok(())
    }
    fn evaluate<'js>(
        _: &Ctx<'js>,
        exports: &rquickjs::module::Exports<'js>,
    ) -> rquickjs::Result<()> {
        exports.export("collectSync", js_collect_sync)?;
        Ok(())
    }
}
