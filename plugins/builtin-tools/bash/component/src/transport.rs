use super::{CommandOutput, Invocation, MAX_ATTACHMENT_BYTES, failure};
use golem_rust::{
    bindings::golem::tool::host,
    schema::{render::json_value::to_json_value_redacted, wit::wire::ToolRpcError},
};

/// Cancels an accepted tool operation if its caller disappears before the result arrives, for
/// example when `kill` or the end of a `run` terminates the shell process that started it.
/// Reader closure never reaches this: a bound tool writes its output only after completion.
struct CancelOnDrop(Option<host::FutureInvokeResult>);

impl CancelOnDrop {
    async fn get(mut self) -> Result<host::InvocationResult, ToolRpcError> {
        let result = match &self.0 {
            Some(future) => future.get().await,
            None => unreachable!("the guard holds its future until completion"),
        };
        self.0 = None;
        result
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(future) = self.0.take() {
            future.cancel();
        }
    }
}

pub(super) async fn invoke(call: &Invocation, stdin: Option<Vec<u8>>) -> CommandOutput {
    let input = match golem_rust::encode_typed_schema_value(&call.input) {
        Ok(input) => input,
        Err(error) => return failure(2, format!("invalid canonical input: {error:?}")),
    };
    let rpc = host::ToolRpc::new(&call.name);
    let (writer, stdin) = match stdin {
        Some(bytes) => {
            if bytes.len() > MAX_ATTACHMENT_BYTES {
                log::warn!("{}: stdin attachment limit exceeded", call.path.join(" "));
                return failure(1, "stdin attachment limit exceeded");
            }
            let (writer, source, watcher) = host::create_stdin();
            // No native read is outstanding: the finite pump owns the bytes.
            drop(watcher);
            (Some((writer, bytes)), Some(source))
        }
        None => (None, None),
    };
    let (stdout, reader) = if call.stdout {
        let (target, reader) = host::create_stdout();
        (Some(target), Some(reader))
    } else {
        (None, None)
    };
    let pump = async {
        if let Some((writer, bytes)) = writer {
            for chunk in bytes.chunks(64 * 1024) {
                writer
                    .write(chunk.to_vec())
                    .await
                    .map_err(|e| format!("stdin: {e:?}"))?;
            }
            writer.finish().await.map_err(|e| format!("stdin: {e:?}"))?;
        }
        Ok::<_, String>(())
    };
    let drain = async {
        let mut bytes = Vec::new();
        let mut failure = None;
        if let Some(mut reader) = reader {
            while let Some(item) = reader.next().await {
                let chunk = match item {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        failure = Some(format!("stdout: {error:?}"));
                        break;
                    }
                };
                let remaining = MAX_ATTACHMENT_BYTES - bytes.len();
                bytes.extend(&chunk[..chunk.len().min(remaining)]);
                if chunk.len() > remaining {
                    failure = Some("stdout attachment limit exceeded".to_string());
                    break;
                }
            }
        }
        (bytes, failure)
    };
    let (terminal, pumped, (stdout, drain_failure)) = futures::join!(
        CancelOnDrop(Some(
            rpc.async_invoke_and_await(&call.path, input, stdin, stdout)
        ))
        .get(),
        pump,
        drain
    );
    let mut output = CommandOutput {
        stdout,
        ..Default::default()
    };
    if let Some(error) = drain_failure {
        log::warn!("{}: {error}", call.path.join(" "));
        output.stderr = format!("{error}\n").into_bytes();
        output.exit_code = 1;
    }
    match terminal {
        Ok(result) => {
            // A declared stdout is authoritative even when the stream is empty.
            if !call.stdout
                && let Some(wire) = result.result
            {
                let typed = match golem_rust::decode_typed_schema_value(&wire) {
                    Ok(typed) => typed,
                    Err(error) => return failure(1, format!("result: {error:?}")),
                };
                let json =
                    match to_json_value_redacted(typed.graph(), &typed.graph().root, typed.value())
                    {
                        Ok(json) => json,
                        Err(error) => return failure(1, format!("result: {error:?}")),
                    };
                output.stdout = match json {
                    serde_json::Value::String(text) => text.into_bytes(),
                    value => format!("{value}\n").into_bytes(),
                };
            }
            if let Err(e) = pumped {
                output.stderr.extend(format!("{e}\n").bytes());
                output.exit_code = 1;
            }
        }
        Err(error) => {
            let (code, message) = super::rpc_failure(error, &call.errors);
            log::warn!("{}: exit {code}: {message}", call.path.join(" "));
            output.exit_code = code;
            output.stderr.extend(format!("{message}\n").bytes());
        }
    }
    output
}
