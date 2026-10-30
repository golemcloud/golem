use super::{CommandOutput, Invocation, MAX_ATTACHMENT_BYTES, OutputFailure, failure};
use golem_rust::{
    bindings::golem::tool::{host, streams},
    schema::wit::wire::ToolRpcError,
};

type OutputReader = wit_bindgen::StreamReader<Result<Vec<u8>, streams::ByteStreamFailure>>;

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
        let (target, reader) = host::create_output();
        (Some(target), Some(reader))
    } else {
        (None, None)
    };
    // Attached only when the tool declares stderr: the host rejects an undeclared attachment.
    let (stderr, error_reader) = if call.stderr {
        let (target, reader) = host::create_output();
        (Some(target), Some(reader))
    } else {
        (None, None)
    };
    let pump = async {
        if let Some((writer, bytes)) = writer {
            for chunk in bytes.chunks(64 * 1024) {
                writer.write(chunk.to_vec()).await?;
            }
            writer.finish().await?;
        }
        Ok::<_, streams::StreamWriteError>(())
    };
    // Every reader is drained in the same join as the terminal wait: an undrained one would stall
    // the provider once its buffer fills.
    let (terminal, pumped, (stdout, stdout_failure), (stderr, stderr_failure)) = futures::join!(
        CancelOnDrop(Some(
            rpc.async_invoke_and_await(&call.path, input, stdin, stdout, stderr)
        ))
        .get(),
        pump,
        drain(reader),
        drain(error_reader)
    );
    let command = super::command_label(&call.name, &call.path);
    let terminal = terminal.map_err(|error| super::rpc_failure(error, &call.errors));
    let call_error = terminal.as_ref().err().map(|(_, message)| message.as_str());
    // The provider's own stderr comes first, then the shell's diagnostics about the call.
    let mut output = CommandOutput {
        stdout,
        stderr,
        ..Default::default()
    };
    for (channel, failure) in [("stdout", stdout_failure), ("stderr", stderr_failure)] {
        if let Some(failure) = failure {
            let line = super::output_failure_line(&command, channel, &failure, call_error);
            log::warn!("{line}");
            output.stderr.extend(format!("{line}\n").bytes());
            output.exit_code = 1;
        }
    }
    match terminal {
        Ok(result) => {
            super::apply_result(
                &mut output,
                call.stdout,
                call.result_is_status,
                result.result,
            );
            if let Err(error) = pumped {
                let line = super::stdin_failure_line(&command, &error);
                log::warn!("{line}");
                output.stderr.extend(format!("{line}\n").bytes());
                output.exit_code = 1;
            }
        }
        Err((code, message)) => {
            log::warn!("{command}: exit {code}: {message}");
            output.exit_code = code;
            output.stderr.extend(format!("{message}\n").bytes());
        }
    }
    output
}

/// Reads one declared output channel to its end, keeping at most [`MAX_ATTACHMENT_BYTES`].
async fn drain(reader: Option<OutputReader>) -> (Vec<u8>, Option<OutputFailure>) {
    let mut bytes = Vec::new();
    let Some(mut reader) = reader else {
        return (bytes, None);
    };
    while let Some(item) = reader.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => return (bytes, Some(OutputFailure::Stream(error))),
        };
        let remaining = MAX_ATTACHMENT_BYTES - bytes.len();
        bytes.extend(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining {
            return (bytes, Some(OutputFailure::LimitExceeded));
        }
    }
    (bytes, None)
}
