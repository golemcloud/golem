//! Pending invocation fixtures exercised through the actual WASM shell command dispatcher.
#![allow(
    clippy::unwrap_used,
    reason = "acceptance fixture metadata is fixed and validated"
)]
use bash_shell::commands::{
    CommandDescriptor, CommandFuture, CommandInvoker, CommandOutput, PreparedCommand,
};
use brush_core::builtins::{ContentOptions, ContentType, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::{DefaultShellExtensions, ShellExtensions};
use brush_core::{Error, ExecutionResult};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static LEGACY_EXECUTIONS: AtomicUsize = AtomicUsize::new(0);
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
