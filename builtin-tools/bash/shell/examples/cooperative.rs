//! Standalone acceptance driver for cooperative WASM pipelines.
//!
//! `cooperative [--status-file PATH] [--time-limit SECONDS] [--calls] SCRIPT...` runs SCRIPT in a
//! fresh shell. With `--calls`, each SCRIPT is a separate call, as the bash tool's `run` makes
//! them: a fresh shell with its own `$$`, started in the directory the previous call ended in, and
//! nothing else kept. Output is concatenated and the status is the last call's. `--time-limit`
//! gives each call the limit the tool's `timeout` argument gives it.
use std::io::Write;
#[cfg(all(target_arch = "wasm32", feature = "test-support"))]
#[path = "support/probe_tools.rs"]
mod probe_tools;

use bash_shell::session::Session;

/// Call `index`'s `$$`. The tool draws a random one per call; here the numbers are fixed so the
/// matrix's outputs are reproducible, and distinct so each call is visibly a new process. The
/// first call keeps a new shell's own numbering, 1.
fn shell_pid(index: usize) -> i32 {
    1 + 1_000 * i32::try_from(index).unwrap_or(0)
}

/// The task runtime the component gives the shell (`component/src/execution.rs`): the component
/// model's own tasks and sleeps. It is the only runtime that receives the completions of the
/// WASI-HTTP futures `curl` and `wget` await; under Tokio's, the first request panicked with no
/// task to register them with.
#[cfg(target_arch = "wasm32")]
fn services() -> bash_shell::ExecutionServices {
    use wcurl::wasip3;
    bash_shell::ExecutionServices {
        spawn_local: tasks::spawn,
        yield_now: || Box::pin(wasip3::wit_bindgen::yield_async()),
        sleep: |duration| {
            Box::pin(async move {
                let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
                wasip3::clocks::monotonic_clock::wait_for(nanos).await;
            })
        },
    }
}

/// The shell's spawned tasks, run as the component's runtime runs them. The component spawns with
/// its own wit-bindgen (0.59), which keeps its export's future and every spawned task in one
/// queue, polled in turn, and polls a task spawned during a poll before it lets the component
/// wait. The 0.57 that WASI-HTTP's bindings bring here does neither: a pipeline's stages would sit
/// unpolled while the task waited for a host event that never came. So this is 0.59's loop.
#[cfg(target_arch = "wasm32")]
mod tasks {
    use futures::future::LocalBoxFuture;
    use futures::stream::{FuturesUnordered, StreamExt};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::Poll;

    thread_local! {
        static SPAWNED: RefCell<Vec<LocalBoxFuture<'static, ()>>> =
            const { RefCell::new(Vec::new()) };
    }

    pub(super) fn spawn(future: LocalBoxFuture<'static, ()>) {
        SPAWNED.with_borrow_mut(|spawned| spawned.push(future));
    }

    /// Runs `main` to completion among the tasks spawned meanwhile; those still running when it
    /// ends are dropped, as a Tokio `LocalSet` drops them.
    pub(super) async fn run<'a, T: 'a>(main: impl Future<Output = T> + 'a) -> T {
        let result = Rc::new(RefCell::new(None));
        let slot = Rc::clone(&result);
        let mut tasks: FuturesUnordered<LocalBoxFuture<'a, ()>> = FuturesUnordered::new();
        tasks.push(Box::pin(async move {
            let value = main.await;
            *slot.borrow_mut() = Some(value);
        }));
        std::future::poll_fn(move |cx| {
            loop {
                let poll = tasks.poll_next_unpin(cx);
                let spawned = SPAWNED.with_borrow_mut(std::mem::take);
                let any_spawned = !spawned.is_empty();
                tasks.extend(spawned);
                if let Some(value) = result.borrow_mut().take() {
                    return Poll::Ready(value);
                }
                if !(poll.is_ready() || any_spawned) {
                    return Poll::Pending;
                }
            }
        })
        .await
    }
}

/// Run one call; returns its stdout, stderr, status and the directory it ended in.
async fn call(
    mut shell: Session,
    script: &str,
    index: usize,
    cwd: Option<&str>,
    time_limit: Option<std::time::Duration>,
) -> Result<(Vec<u8>, Vec<u8>, u8, String), String> {
    let pid = shell_pid(index);
    shell.seed_process_ids(pid, pid + 1);
    if let Some(limit) = time_limit {
        shell.set_time_limit(limit);
    }
    if let Some(cwd) = cwd {
        shell.set_working_dir(cwd)?;
    }
    let result = shell.run(script).await;
    Ok((result.stdout, result.stderr, result.exit_code, result.cwd))
}

/// The command line: where to report the status, the per-call time limit, and the scripts.
struct Invocation {
    status_file: Option<String>,
    time_limit: Option<std::time::Duration>,
    scripts: Vec<String>,
}

fn parse_args() -> Result<Invocation, Box<dyn std::error::Error>> {
    const USAGE: &str =
        "usage: cooperative [--status-file PATH] [--time-limit SECONDS] [--calls] SCRIPT...";
    // WASI's exit status only says success or failure, so the shell's own status is written to a
    // file for the harness. It is an argument, not a variable, so scripts never see it.
    let mut args = std::env::args().skip(1).peekable();
    let mut status_file = None;
    if args.peek().map(String::as_str) == Some("--status-file") {
        args.next();
        status_file = Some(args.next().ok_or(USAGE)?);
    }
    let mut time_limit = None;
    if args.peek().map(String::as_str) == Some("--time-limit") {
        args.next();
        let seconds: f64 = args.next().ok_or(USAGE)?.parse().map_err(|_| USAGE)?;
        time_limit = Some(std::time::Duration::try_from_secs_f64(seconds).map_err(|_| USAGE)?);
    }
    let calls = args.peek().map(String::as_str) == Some("--calls");
    if calls {
        args.next();
    }
    let scripts: Vec<String> = args.collect();
    if scripts.is_empty() || (!calls && scripts.len() > 1) {
        return Err(USAGE.into());
    }
    Ok(Invocation {
        status_file,
        time_limit,
        scripts,
    })
}

/// Runs every call in turn, each in a fresh shell from `new_shell`; returns the concatenated
/// output and the last call's status.
async fn drive<F, S>(
    invocation: &Invocation,
    new_shell: F,
) -> Result<(Vec<u8>, Vec<u8>, u8), Box<dyn std::error::Error>>
where
    F: Fn() -> S,
    S: std::future::Future<Output = Result<Session, Box<dyn std::error::Error>>>,
{
    let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), 0);
    let mut cwd: Option<String> = None;
    for (index, script) in invocation.scripts.iter().enumerate() {
        let shell = new_shell().await?;
        let (out, err, code, next) =
            call(shell, script, index, cwd.as_deref(), invocation.time_limit).await?;
        #[cfg(all(target_arch = "wasm32", feature = "test-support"))]
        probe_tools::assert_quiescent();
        stdout.extend(out);
        stderr.extend(err);
        status = code;
        cwd = Some(next);
    }
    Ok((stdout, stderr, status))
}

/// Writes the output and the status report, and exits with WASI's success or failure.
fn finish(invocation: &Invocation, stdout: &[u8], stderr: &[u8], status: u8) -> ! {
    let written = std::io::stdout()
        .write_all(stdout)
        .and_then(|()| std::io::stderr().write_all(stderr))
        .and_then(|()| match &invocation.status_file {
            Some(path) => std::fs::write(path, status.to_string()),
            None => Ok(()),
        });
    if let Err(error) = written {
        eprintln!("cooperative: {error}");
        std::process::exit(1);
    }
    std::process::exit(i32::from(status));
}

/// On wasm the driver is the component's async `wasi:cli/run@0.3.0` export, which `wasmtime run
/// -Sp3` prefers to `main`: WASI-HTTP's futures can be awaited only in an async task, as the
/// tool's own `run` export awaits them, and a synchronous `main` may not block on them.
#[cfg(target_arch = "wasm32")]
struct Command;

#[cfg(target_arch = "wasm32")]
impl wcurl::wasip3::exports::wasi::cli::run::Guest for Command {
    async fn run() -> Result<(), ()> {
        let outcome = match parse_args() {
            Ok(invocation) => tasks::run(drive(&invocation, || async {
                #[cfg_attr(not(feature = "test-support"), allow(unused_mut))]
                let mut shell = Session::new_with_execution_services(services()).await?;
                #[cfg(feature = "test-support")]
                probe_tools::install(&mut shell);
                Ok(shell)
            }))
            .await
            .map(|(stdout, stderr, status)| (invocation, stdout, stderr, status)),
            Err(error) => Err(error),
        };
        match outcome {
            Ok((invocation, stdout, stderr, status)) => {
                finish(&invocation, &stdout, &stderr, status)
            }
            Err(error) => {
                eprintln!("cooperative: {error}");
                Err(())
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
wcurl::wasip3::cli::command::export!(Command);

/// Unused on wasm, where `wasmtime run -Sp3` calls the async export above instead.
#[cfg(target_arch = "wasm32")]
fn main() {
    eprintln!("cooperative: run it with `wasmtime run -Sp3`, which calls its async export");
    std::process::exit(1);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let invocation = parse_args()?;
    // Natively, command substitutions use Tokio's pipes, which need its IO driver.
    let (stdout, stderr, status) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(drive(&invocation, || async {
            Ok::<_, Box<dyn std::error::Error>>(Session::new().await?)
        }))?;
    finish(&invocation, &stdout, &stderr, status)
}
