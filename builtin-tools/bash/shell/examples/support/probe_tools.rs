//! Pending invocation fixtures exercised through the actual WASM shell command dispatcher.
#![allow(
    clippy::unwrap_used,
    reason = "acceptance fixture metadata is fixed and validated"
)]
use bash_shell::commands::{
    CommandDescriptor, CommandFuture, CommandInvoker, CommandOutput, FileEditor, FileReader,
    FileWriter, PreparedCommand,
};
use brush_core::builtins::{ContentOptions, ContentType, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::{DefaultShellExtensions, ShellExtensions};
use brush_core::{Error, ExecutionResult};
use std::{
    path::{Component, Path},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static LEGACY_EXECUTIONS: AtomicUsize = AtomicUsize::new(0);
static FORWARDED_READS: AtomicUsize = AtomicUsize::new(0);
static FORWARDED_WRITES: AtomicUsize = AtomicUsize::new(0);
static FORWARDED_EDITS: AtomicUsize = AtomicUsize::new(0);
/// The largest file that a stand-in for a file tool takes, as for the tool's own limit.
const MAX_FORWARDED_BYTES: usize = 1024 * 1024;
const COMPLETION_MARKER: &str = "/tmp/probe-completed";
struct PendingCall;
impl Drop for PendingCall {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn assert_quiescent() {
    assert_eq!(
        ACTIVE.load(Ordering::SeqCst),
        0,
        "tool future outlived shell evaluation"
    );
    assert_eq!(
        LEGACY_EXECUTIONS.load(Ordering::SeqCst),
        0,
        "obsolete synchronous-stdin callback executed"
    );
}

/// A deliberately obsolete synchronous input adapter. Cooperative input must be rejected before
/// this callback runs, including when the producer has already reached EOF.
struct LegacyStdinProbe;

impl SimpleCommand for LegacyStdinProbe {
    fn get_content(
        _name: &str,
        _content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        Ok(String::new())
    }

    fn execute<SE, I, S>(
        _context: ExecutionContext<'_, SE>,
        _args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        LEGACY_EXECUTIONS.fetch_add(1, Ordering::SeqCst);
        Ok(ExecutionResult::success())
    }
}

struct ProbeInvoker;
struct Prepared(String);
impl CommandInvoker for ProbeInvoker {
    fn prepare(&self, _: &str, argv: &[String]) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
        let name = argv.first().map(String::as_str).unwrap_or("--help");
        if name == "--help" {
            return Err(CommandOutput {
                stdout: b"probe-tool: delay input fail destroy fail-output\n".to_vec(),
                ..Default::default()
            });
        }
        if argv.len() != 1 || !["delay", "input", "fail", "destroy", "fail-output"].contains(&name)
        {
            return Err(CommandOutput {
                stderr: b"probe-tool: invalid command\n".to_vec(),
                exit_code: 2,
                ..Default::default()
            });
        }
        Ok(Box::new(Prepared(name.to_owned())))
    }
}
impl PreparedCommand for Prepared {
    fn takes_stdin(&self) -> bool {
        self.0 == "input"
    }
    fn invoke(&self, stdin: Option<Vec<u8>>) -> CommandFuture<'_> {
        Box::pin(async move {
            match self.0.as_str() {
                "delay" => {
                    ACTIVE.fetch_add(1, Ordering::SeqCst);
                    let _pending = PendingCall;
                    let nanos = u64::try_from(Duration::from_millis(100).as_nanos()).unwrap();
                    wcurl::wasip3::clocks::monotonic_clock::wait_for(nanos).await;
                    std::fs::write(COMPLETION_MARKER, b"completed\n")
                        .expect("write completion marker");
                    let peer = std::fs::read("/tmp/probe-peer").is_ok();
                    CommandOutput {
                        stdout: format!("completed:peer={peer}\n").into_bytes(),
                        ..Default::default()
                    }
                }
                "input" => CommandOutput {
                    stdout: stdin.unwrap_or_default(),
                    ..Default::default()
                },
                "fail" => CommandOutput {
                    stderr: b"named fixture error\n".to_vec(),
                    exit_code: 7,
                    ..Default::default()
                },
                "fail-output" => CommandOutput {
                    stdout: b"partial\n".to_vec(),
                    exit_code: 7,
                    ..Default::default()
                },
                "destroy" => CommandOutput {
                    stdout: b"destroyed\n".to_vec(),
                    ..Default::default()
                },
                _ => unreachable!("command validated during preparation"),
            }
        })
    }
}
/// Whether the file tools take `path`: it is UTF-8 and has no parent component.
fn tool_path(path: &Path) -> bool {
    path.to_str().is_some() && !path.components().any(|part| part == Component::ParentDir)
}

/// A file reader that answers as one that asks the `read-file` tool: each byte of a text file,
/// and nothing for a path with a parent component, a file above the size limit, and content that
/// has a NUL byte or is not UTF-8.
struct ProbeFileReader;

impl FileReader for ProbeFileReader {
    fn read(&self, path: &Path, len: u64) -> Option<Vec<u8>> {
        if len > MAX_FORWARDED_BYTES as u64 || !tool_path(path) {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            return None;
        }
        FORWARDED_READS.fetch_add(1, Ordering::SeqCst);
        Some(bytes)
    }
}

/// A file writer that answers as one that asks the `write-file` tool: it makes a text the
/// content of the file, and it refuses a path with a parent component, content above the size
/// limit, and content that is not UTF-8.
struct ProbeFileWriter;

impl FileWriter for ProbeFileWriter {
    fn max_len(&self) -> usize {
        MAX_FORWARDED_BYTES
    }

    fn write(&self, path: &Path, content: &[u8]) -> bool {
        if content.len() > MAX_FORWARDED_BYTES
            || !tool_path(path)
            || std::str::from_utf8(content).is_err()
            || std::fs::write(path, content).is_err()
        {
            return false;
        }
        FORWARDED_WRITES.fetch_add(1, Ordering::SeqCst);
        true
    }
}

/// A file editor that answers as one that asks the `edit-file` tool: it changes the content of
/// a text file in place, and it refuses a path with a parent component, content above the size
/// limit, a file that has no content or has a NUL byte, and content that is not UTF-8.
struct ProbeFileEditor;

impl FileEditor for ProbeFileEditor {
    fn max_len(&self) -> usize {
        MAX_FORWARDED_BYTES
    }

    fn edit(&self, path: &Path, old: &[u8], new: &[u8]) -> bool {
        if old.is_empty()
            || old.len().max(new.len()) > MAX_FORWARDED_BYTES
            || !tool_path(path)
            || old.contains(&0)
            || std::str::from_utf8(old).is_err()
            || std::str::from_utf8(new).is_err()
            || std::fs::write(path, new).is_err()
        {
            return false;
        }
        FORWARDED_EDITS.fetch_add(1, Ordering::SeqCst);
        true
    }
}

/// Gives the session a stand-in for each bound file tool that a local command uses.
pub(super) fn use_file_tools(session: &mut bash_shell::session::Session) {
    session.forward_file_reads(Arc::new(ProbeFileReader));
    session.forward_file_writes(Arc::new(ProbeFileWriter));
    session.forward_file_edits(Arc::new(ProbeFileEditor));
}

/// How many times each stand-in did its work: one line `KIND COUNT` for each kind of work.
pub(super) fn file_tool_report() -> String {
    format!(
        "reads {}\nwrites {}\nedits {}\n",
        FORWARDED_READS.load(Ordering::SeqCst),
        FORWARDED_WRITES.load(Ordering::SeqCst),
        FORWARDED_EDITS.load(Ordering::SeqCst)
    )
}

pub(super) fn install(session: &mut bash_shell::session::Session) {
    #[allow(deprecated)]
    let registration = brush_core::builtins::simple_builtin_reading_stdin::<
        LegacyStdinProbe,
        DefaultShellExtensions,
    >();
    session.register_test_builtin("legacy-stdin", registration);
    session
        .register_commands(
            vec![CommandDescriptor {
                name: "probe-tool".into(),
                help: "probe-tool: delay input fail destroy fail-output".into(),
            }],
            Arc::new(ProbeInvoker),
        )
        .unwrap();
}
