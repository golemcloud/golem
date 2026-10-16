//! Python CLI execution in a fresh interpreter with the invoking agent's capabilities.

use golem_rust::agentic::{InputStream, OutputStream};
use golem_rust::tool::ProcessExitStatus;
use golem_rust::{ToolError, tool_definition, tool_implementation};
use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::rc::Rc;

mod runtime;

#[derive(Debug, Clone, ToolError)]
pub enum PythonError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Io { reason: String },
}

#[tool_definition(version = "0.1.1", requires_filesystem = true)]
pub trait Python {
    /// Run Python with -c, -m, a script path, or noninteractive stdin. The frozen RustPython
    /// standard library is included. Native extensions, subprocesses and sockets are unavailable.
    #[arg(cwd = "option", default = "/", env = "PWD")]
    #[arg(args = "tail", separator = "--", verbatim = true)]
    #[arg(stderr, channel = "stderr")]
    async fn python(
        &self,
        cwd: String,
        args: Vec<String>,
        stdin: Option<InputStream>,
        stdout: OutputStream,
        stderr: OutputStream,
    ) -> Result<ProcessExitStatus, PythonError>;
}

struct PythonImpl;

#[tool_implementation]
impl Python for PythonImpl {
    async fn python(
        &self,
        cwd: String,
        args: Vec<String>,
        stdin: Option<InputStream>,
        stdout: OutputStream,
        stderr: OutputStream,
    ) -> Result<ProcessExitStatus, PythonError> {
        let streams = Rc::new(RefCell::new(runtime::Stdio::new(
            Box::new(AttachedInput {
                stream: stdin,
                pending: io::Cursor::new(Vec::new()),
            }),
            Box::new(AttachedOutput(stdout.clone())),
            Box::new(AttachedOutput(stderr.clone())),
        )));
        let result = runtime::run(&cwd, &args, streams);
        // Keep both attachments alive through interpreter shutdown and atexit handlers.
        let out = stdout.finish().await;
        let err = stderr.finish().await;
        out.map_err(|e| PythonError::Io {
            reason: format!("stdout: {e:?}"),
        })?;
        err.map_err(|e| PythonError::Io {
            reason: format!("stderr: {e:?}"),
        })?;
        result
            .map(|exit_code| ProcessExitStatus {
                exit_code: exit_code as u8,
            })
            .map_err(|reason| PythonError::Io { reason })
    }
}

struct AttachedInput {
    stream: Option<InputStream>,
    pending: io::Cursor<Vec<u8>>,
}

impl Read for AttachedInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        #[cfg(target_arch = "wasm32")]
        loop {
            let count = self.pending.read(buffer)?;
            if count != 0 {
                return Ok(count);
            }
            let Some(stream) = &mut self.stream else {
                return Ok(0);
            };
            match wit_bindgen::block_on(stream.next()) {
                Some(Ok(bytes)) => self.pending = io::Cursor::new(bytes),
                Some(Err(
                    golem_rust::golem_agentic::golem::tool::streams::ByteStreamFailure::Cancelled,
                )) => {
                    return Err(io::Error::other(runtime::StdinCancelled));
                }
                Some(Err(error)) => return Err(io::Error::other(format!("stdin: {error:?}"))),
                None => {
                    self.stream = None;
                    return Ok(0);
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let count = self.pending.read(buffer)?;
            if count != 0 || self.stream.is_none() {
                Ok(count)
            } else {
                Err(io::Error::other("tool attachments require WASM"))
            }
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct AttachedOutput(OutputStream);

impl Write for AttachedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(target_arch = "wasm32")]
        for chunk in bytes.chunks(64 * 1024) {
            wit_bindgen::block_on(self.0.write(chunk.to_vec()))
                .map_err(|e| io::Error::other(format!("output: {e:?}")))?;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !bytes.is_empty() {
            return Err(io::Error::other("tool attachments require WASM"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
