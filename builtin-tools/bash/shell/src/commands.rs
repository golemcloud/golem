//! Invocation-owned adapter for embedding typed tools behind ordinary shell commands.
use std::{collections::HashMap, sync::Arc};

use brush_core::{
    CommandArg, Error, ExecutionContext, ExecutionResult,
    builtins::{BoxFuture, ContentOptions, ContentType, Registration, SimpleCommand},
};

pub const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;
pub type CommandFuture<'a> = BoxFuture<'a, CommandOutput>;

#[derive(Clone, Debug, Default)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: u8,
}

#[derive(Clone, Debug)]
pub struct CommandDescriptor {
    pub name: String,
    pub help: String,
}

/// Parse arguments and resolve help before any input is consumed.
pub trait CommandInvoker: Send + Sync {
    fn prepare(
        &self,
        name: &str,
        argv: &[String],
    ) -> Result<Box<dyn PreparedCommand>, CommandOutput>;
}

pub trait PreparedCommand: Send + Sync {
    fn takes_stdin(&self) -> bool;
    fn invoke(&self, stdin: Option<Vec<u8>>) -> CommandFuture<'_>;
}

#[derive(Clone, Default)]
pub(crate) struct InvocationCommands {
    entries: HashMap<String, (CommandDescriptor, Arc<dyn CommandInvoker>)>,
}
impl InvocationCommands {
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }
    pub fn insert(&mut self, descriptor: CommandDescriptor, invoker: Arc<dyn CommandInvoker>) {
        self.entries
            .insert(descriptor.name.clone(), (descriptor, invoker));
    }
}

struct BoundCommand;
impl SimpleCommand for BoundCommand {
    fn get_content(name: &str, _: ContentType, _: &ContentOptions) -> Result<String, Error> {
        Ok(format!("{name}: bound command; run {name} --help\n"))
    }
    fn execute<SE: brush_core::ShellExtensions, I: Iterator<Item = S>, S: AsRef<str>>(
        _: ExecutionContext<'_, SE>,
        _: I,
    ) -> Result<ExecutionResult, Error> {
        unreachable!("bound commands use their asynchronous execution registration")
    }
}

pub(crate) fn registration() -> Registration<brush_core::extensions::DefaultShellExtensions> {
    let mut registration = brush_core::builtins::simple_builtin::<
        BoundCommand,
        brush_core::extensions::DefaultShellExtensions,
    >();
    registration.execute_func = execute;
    #[cfg(target_arch = "wasm32")]
    {
        registration.execution_boundary = brush_core::builtins::ExecutionBoundary::Command;
    }
    registration
}

/// Run the bound tool registered under this command's name, for a local command that forwards
/// some of its invocations (`bash run …`, see `tools::sh`).
pub(crate) fn forward(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    execute(context, args)
}

fn execute(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let argv = args
            .iter()
            .skip(1)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let runtime = context.params.context::<InvocationCommands>();
        let prepared = runtime
            .as_ref()
            .and_then(|runtime| runtime.entries.get(&context.command_name))
            .map(|(_, invoker)| invoker.prepare(&context.command_name, &argv));
        let outcome = match prepared {
            Some(Ok(prepared)) => {
                let stdin = if prepared.takes_stdin() {
                    match read_stdin(&context).await {
                        Ok(stdin) => Some(stdin),
                        Err(error) => {
                            return write_output(
                                &context,
                                CommandOutput {
                                    stderr: format!("{}: {error}\n", context.command_name)
                                        .into_bytes(),
                                    exit_code: 2,
                                    ..Default::default()
                                },
                            )
                            .await;
                        }
                    }
                } else {
                    None
                };
                prepared.invoke(stdin).await
            }
            Some(Err(output)) => output,
            None => CommandOutput {
                stderr: format!("{}: command is not bound\n", context.command_name).into_bytes(),
                exit_code: 127,
                ..Default::default()
            },
        };
        write_output(&context, outcome).await
    })
}

async fn read_stdin(context: &ExecutionContext<'_>) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    #[cfg(target_arch = "wasm32")]
    {
        use futures::io::AsyncReadExt;
        crate::tools::streaming::input(context)
            .async_io()
            .take((MAX_STDIN_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::io::Read;
        context
            .stdin()
            .take((MAX_STDIN_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
    }
    if bytes.len() > MAX_STDIN_BYTES {
        return Err("stdin exceeds the 16 MiB command input limit".into());
    }
    Ok(bytes)
}

async fn write_output(
    context: &ExecutionContext<'_>,
    output: CommandOutput,
) -> Result<ExecutionResult, Error> {
    write_output_in_order(context, output, false).await
}

/// Writes a command's output and diagnostics: its stdout first, or (`diagnostics_first`) its
/// stderr first, as a command whose stdout is buffered to its end shows them.
async fn write_output_in_order(
    context: &ExecutionContext<'_>,
    output: CommandOutput,
    diagnostics_first: bool,
) -> Result<ExecutionResult, Error> {
    let write = async {
        #[cfg(target_arch = "wasm32")]
        {
            use futures::io::AsyncWriteExt;
            if diagnostics_first {
                context
                    .stderr()
                    .async_io()
                    .write_all(&output.stderr)
                    .await?;
            }
            context
                .stdout()
                .async_io()
                .write_all(&output.stdout)
                .await?;
            if !diagnostics_first {
                context
                    .stderr()
                    .async_io()
                    .write_all(&output.stderr)
                    .await?;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::io::Write;
            if diagnostics_first {
                context.stderr().write_all(&output.stderr)?;
            }
            context.stdout().write_all(&output.stdout)?;
            if !diagnostics_first {
                context.stderr().write_all(&output.stderr)?;
            }
        }
        Ok::<_, std::io::Error>(())
    }
    .await;
    if let Err(error) = write {
        #[cfg(target_arch = "wasm32")]
        if error.kind() == std::io::ErrorKind::BrokenPipe
            && matches!(
                brush_core::execution::process::pipe_disposition(),
                brush_core::traps::PipeDisposition::Ignored
            )
        {
            use futures::io::AsyncWriteExt;
            let _ = context
                .stderr()
                .async_io()
                .write_all(
                    format!("{}: write error: Broken pipe\n", context.command_name).as_bytes(),
                )
                .await;
            return Ok(ExecutionResult::new(1));
        }
        return Err(error.into());
    }
    Ok(ExecutionResult::new(output.exit_code))
}

pub(crate) fn http_registration() -> Registration<brush_core::extensions::DefaultShellExtensions> {
    let mut registration = registration();
    registration.execute_func = execute_http;
    registration.content_func = http_content;
    registration
}

fn http_content(name: &str, _: ContentType, _: &ContentOptions) -> Result<String, Error> {
    Ok(if name == "curl" {
        "curl URL... [-o FILE] [-O] [-J] [--create-dirs] [-s] [-S] [-L] [-i] [-I] [-f] \
         [--fail-with-body] [-X METHOD] [-d DATA|@FILE] [--data-urlencode DATA] [--json DATA] \
         [-G] [-F name=value|name=@file[;type=CT]] [-T FILE] [-H 'K: V'] [-A USER_AGENT] \
         [-u USER:PASS] [-e REFERER] [-r RANGE] [-b COOKIES|FILE] [-c FILE] [--compressed] \
         [-m SECONDS] [--connect-timeout SECONDS] [--retry N] [--retry-delay SECONDS] \
         [-w FORMAT] [-K FILE] [-k] [-v] [-V]\n\
         Fetch HTTP data to stdout or a file. -L follows redirects; -f fails on HTTP errors; -O \
         saves under the URL's basename (-J prefers a Content-Disposition filename); -F sends \
         multipart/form-data; -T PUTs a file; -b/-c read/write a Netscape cookie jar; -K reads \
         options from a file. Each URL is fetched in turn, the n-th -o or -O naming its output. \
         -k cannot skip certificate checks: WASI-HTTP always makes them.\n"
    } else {
        "wget URL... [-i FILE] [-O FILE|-] [-P DIR] [-c] [-nc] [-N] [-q] [-S] [--spider] \
         [-T SECONDS] [-t TRIES] [--max-redirect N] [--post-data DATA|--post-file FILE] \
         [--header 'K: V'] [-U USER_AGENT] [--content-disposition] [-V]\n\
         Download HTTP data to a file, or stdout with -O -. Redirects are followed by default; \
         -c resumes a partial download with a Range request (restarting if the server ignores \
         it); -P sets the destination directory; -nc keeps a file already there; --spider only \
         checks that each URL is there; -i reads URLs from a file; -N is accepted but does not \
         check timestamps. FTP is unsupported.\n"
    }
    .to_owned())
}

fn execute_http(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let argv = args
            .iter()
            .skip(1)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if argv
            .iter()
            .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
        {
            return write_output(
                &context,
                CommandOutput {
                    stdout: http_content(
                        &context.command_name,
                        ContentType::DetailedHelp,
                        &ContentOptions::default(),
                    )?
                    .into_bytes(),
                    ..Default::default()
                },
            )
            .await;
        }
        // Stream the response body straight to this command's own stdout instead of buffering it
        // in a `CommandOutput` first: `OpenFile::async_io()` is a `futures::io::AsyncWrite` on
        // both targets, so `wcurl`/`waget`'s `run_streaming` can write chunks to it as they
        // arrive. `stdout_handle` outlives the streamed call so `sink` (its borrow) stays valid
        // across every `.await` inside it.
        let is_curl = context.command_name == "curl";
        let cwd = context.shell.working_dir();
        let mut stdout_handle = context.stdout();
        let sink = stdout_handle.async_io();
        // `wcurl::Outcome`/`waget::Outcome` are structurally identical but distinct types (each
        // crate is deliberately standalone) — flatten to a common tuple so both arms unify.
        let streamed: Result<(Vec<u8>, Vec<u8>, u8), std::io::Error> = if is_curl {
            wcurl::run_streaming(&argv, cwd, sink)
                .await
                .map(|o| (o.stdout, o.stderr, o.exit_code))
        } else {
            // wget keeps its HSTS store in the `HOME` the script exports.
            let home = crate::tools::coreutils::exported_env(&context)
                .into_iter()
                .find_map(|(name, value)| (name == "HOME").then_some(value));
            waget::run_streaming(&argv, cwd, home.as_deref(), sink)
                .await
                .map(|o| (o.stdout, o.stderr, o.exit_code))
        };
        match streamed {
            // curl's own stdout is buffered to its end and its stderr is not, so its diagnostics
            // come before the `-w` output it prints last.
            Ok((stdout, stderr, exit_code)) => {
                write_output_in_order(
                    &context,
                    CommandOutput {
                        stdout,
                        stderr,
                        exit_code,
                    },
                    is_curl,
                )
                .await
            }
            // A write to `sink` failed — the only way `run_streaming` returns `Err` (see its own
            // doc): every other failure (usage, transport, a local file write under `-o`/`-O`)
            // comes back as `Ok(Outcome)` above. On wasm, with the pipeline's SIGPIPE ignored,
            // that is a real error the command must report itself (mirrors `write_output`'s own
            // BrokenPipe handling below, and `yes`/`seq`'s policy in `tools::streaming`) — curl
            // uses its own wording and exit code (23) for this specific failure; wget falls back
            // to the generic wording every other write-erroring command in this crate uses. With
            // the default disposition, the shell's synthetic-SIGPIPE machinery is what should
            // turn this into the pipeline's usual killed-by-signal status, so the error must
            // still propagate there rather than being swallowed into a plain exit code.
            Err(error) => {
                #[cfg(target_arch = "wasm32")]
                if error.kind() == std::io::ErrorKind::BrokenPipe
                    && matches!(
                        brush_core::execution::process::pipe_disposition(),
                        brush_core::traps::PipeDisposition::Ignored
                    )
                {
                    use futures::io::AsyncWriteExt;
                    let (message, exit_code): (&str, u8) = if is_curl {
                        ("curl: (23) Failure writing output to destination\n", 23)
                    } else {
                        ("wget: write error: Broken pipe\n", 1)
                    };
                    let mut stderr = context.stderr();
                    if let Err(write_error) = stderr.async_io().write_all(message.as_bytes()).await
                        && write_error.kind() != std::io::ErrorKind::BrokenPipe
                    {
                        return Err(write_error.into());
                    }
                    return Ok(ExecutionResult::new(exit_code));
                }
                Err(error.into())
            }
        }
    })
}
