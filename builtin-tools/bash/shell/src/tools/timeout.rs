//! `timeout [OPTION]... DURATION COMMAND [ARG]...`, as GNU coreutils' `timeout(1)`: runs COMMAND
//! as a child of this shell (every command here runs in-process), sends it SIGNAL (TERM) once
//! DURATION has passed, and KILL `--kill-after` later. Status 124 when the command timed out,
//! 137 when it was killed, 125 for timeout's own errors, 126 and 127 when COMMAND cannot run.
use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

use crate::manifest::Manifest;

pub(crate) struct Timeout;

impl Timeout {
    pub(crate) const NAME: &'static str = "timeout";
    pub(crate) const SYNOPSIS: &'static str = "run a command with a time limit";
}

const HELP: &str = "\
Usage: timeout [OPTION]... DURATION COMMAND [ARG]...
Start COMMAND, and kill it if still running after DURATION.

Mandatory arguments to long options are mandatory for short options too.
  -f, --foreground
         when not running timeout directly from a shell prompt,
         allow COMMAND to read from the TTY and get TTY signals;
         in this mode, children of COMMAND will not be timed out
  -k, --kill-after=DURATION
         also send a KILL signal if COMMAND is still running
         this long after the initial signal was sent
  -p, --preserve-status
         exit with the same status as COMMAND,
         even when the command times out
  -s, --signal=SIGNAL
         specify the signal to be sent on timeout;
         SIGNAL may be a name like 'HUP' or a number;
         see 'kill -l' for a list of signals
  -v, --verbose
         diagnose to standard error any signal sent upon timeout
      --help
         display this help and exit
      --version
         output version information and exit

DURATION is a floating point number with an optional suffix:
's' for seconds (the default), 'm' for minutes, 'h' for hours or 'd' for days.
A duration of 0 disables the associated timeout.

Upon timeout, send the TERM signal to COMMAND, if no other SIGNAL specified.
The TERM signal kills any process that does not block or catch that signal.
It may be necessary to use the KILL signal, since this signal can't be caught.

Exit status:
  124  if COMMAND times out, and --preserve-status is not specified
  125  if the timeout command itself fails
  126  if COMMAND is found but cannot be invoked
  127  if COMMAND cannot be found
  137  if COMMAND (or timeout itself) is sent the KILL (9) signal (128+9)
  -    the exit status of COMMAND otherwise
";

const VERSION: &str = "timeout (bash-tool, GNU coreutils compatible)\n";

/// Status when COMMAND timed out.
#[cfg(target_arch = "wasm32")]
const TIMED_OUT: u8 = 124;
/// Status when timeout itself fails.
const FAILED: u8 = 125;
/// The KILL signal's number.
#[cfg(target_arch = "wasm32")]
const KILL: u8 = 9;

/// What `timeout` was asked to do.
#[derive(Debug, PartialEq)]
pub(crate) enum Plan {
    /// Print this text to stdout and exit 0 (`--help`, `--version`).
    Print(&'static str),
    /// Run `command`, stopping it with `signal` after `duration` (none if zero), and KILL
    /// `kill_after` later (none if zero).
    Run {
        duration: std::time::Duration,
        signal: u8,
        kill_after: std::time::Duration,
        preserve_status: bool,
        verbose: bool,
        command: Vec<String>,
    },
}

/// A diagnostic for stderr and the status timeout exits with.
type Refusal = (String, u8);

fn usage(message: Option<String>) -> Refusal {
    let mut text = message
        .map(|m| format!("timeout: {m}\n"))
        .unwrap_or_default();
    text.push_str("Try 'timeout --help' for more information.\n");
    (text, FAILED)
}

/// Parses `timeout`'s arguments (argv[0] excluded) as GNU's getopt does for it: options up to the
/// first operand (`+ksvfp`), long options by any unambiguous prefix.
pub(crate) fn plan(argv: &[String]) -> Result<Plan, Refusal> {
    const LONG: [&str; 7] = [
        "foreground",
        "kill-after",
        "preserve-status",
        "signal",
        "verbose",
        "help",
        "version",
    ];
    let mut signal = 15;
    let mut kill_after = std::time::Duration::ZERO;
    let (mut preserve_status, mut verbose) = (false, false);
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value.to_owned())));
            let matches: Vec<&str> = LONG
                .iter()
                .copied()
                .filter(|candidate| candidate.starts_with(name))
                .collect();
            let option = match matches.as_slice() {
                [option] => *option,
                _ if LONG.contains(&name) => name,
                [] => return Err(usage(Some(format!("unrecognized option '--{name}'")))),
                _ => {
                    return Err(usage(Some(format!(
                        "option '--{name}' is ambiguous; possibilities:{}",
                        matches
                            .iter()
                            .map(|option| format!(" '--{option}'"))
                            .collect::<String>()
                    ))));
                }
            };
            let takes_value = matches!(option, "kill-after" | "signal");
            if !takes_value && value.is_some() {
                return Err(usage(Some(format!(
                    "option '--{option}' doesn't allow an argument"
                ))));
            }
            let value = if takes_value {
                match value {
                    Some(value) => Some(value),
                    None => {
                        index += 1;
                        Some(argv.get(index).cloned().ok_or_else(|| {
                            usage(Some(format!("option '--{option}' requires an argument")))
                        })?)
                    }
                }
            } else {
                None
            };
            match (option, value) {
                ("help", _) => return Ok(Plan::Print(HELP)),
                ("version", _) => return Ok(Plan::Print(VERSION)),
                ("foreground", _) => {}
                ("preserve-status", _) => preserve_status = true,
                ("verbose", _) => verbose = true,
                ("kill-after", Some(value)) => kill_after = interval(&value)?,
                ("signal", Some(value)) => signal = signal_number(&value)?,
                _ => {}
            }
            index += 1;
            continue;
        }
        let Some(letters) = arg.strip_prefix('-').filter(|letters| !letters.is_empty()) else {
            break;
        };
        for (at, letter) in letters.char_indices() {
            match letter {
                'f' => {}
                'p' => preserve_status = true,
                'v' => verbose = true,
                'k' | 's' => {
                    let rest = &letters[at + letter.len_utf8()..];
                    let value = if rest.is_empty() {
                        index += 1;
                        argv.get(index).cloned().ok_or_else(|| {
                            usage(Some(format!("option requires an argument -- '{letter}'")))
                        })?
                    } else {
                        rest.to_owned()
                    };
                    if letter == 'k' {
                        kill_after = interval(&value)?;
                    } else {
                        signal = signal_number(&value)?;
                    }
                    break;
                }
                other => return Err(usage(Some(format!("invalid option -- '{other}'")))),
            }
        }
        index += 1;
    }
    let operands = &argv[index.min(argv.len())..];
    let [duration, command @ ..] = operands else {
        return Err(usage(None));
    };
    if command.is_empty() {
        return Err(usage(None));
    }
    Ok(Plan::Run {
        duration: interval(duration)?,
        signal,
        kill_after,
        preserve_status,
        verbose,
        command: command.to_vec(),
    })
}

/// A DURATION: a non-negative floating point number (`.5`, `1e1`, `inf`) with an optional `s`,
/// `m`, `h` or `d` suffix. Durations too long to wait for are taken as forever, as GNU does.
fn interval(text: &str) -> Result<std::time::Duration, Refusal> {
    let invalid = || {
        usage(Some(format!(
            "invalid time interval \u{2018}{text}\u{2019}"
        )))
    };
    let (number, scale) = match text.chars().last() {
        Some('s') => (&text[..text.len() - 1], 1.0),
        Some('m') => (&text[..text.len() - 1], 60.0),
        Some('h') => (&text[..text.len() - 1], 3_600.0),
        Some('d') => (&text[..text.len() - 1], 86_400.0),
        _ => (text, 1.0),
    };
    // Rust's float syntax is C's strtod's but for hexadecimal, and `nan`, which GNU refuses.
    if number.is_empty()
        || number.starts_with(['+', '-'])
        || number.to_ascii_lowercase().contains("nan")
    {
        return Err(invalid());
    }
    let seconds: f64 = number.parse().map_err(|_| invalid())?;
    let seconds = seconds * scale;
    if seconds < 0.0 {
        return Err(invalid());
    }
    Ok(std::time::Duration::try_from_secs_f64(seconds).unwrap_or(std::time::Duration::MAX))
}

/// A SIGNAL: a name (any case, with or without `SIG`) or a number, as `kill` takes it.
fn signal_number(text: &str) -> Result<u8, Refusal> {
    use brush_core::traps::TrapSignal;
    let invalid = || usage(Some(format!("\u{2018}{text}\u{2019}: invalid signal")));
    let number = if let Ok(number) = text.parse::<i32>() {
        if number == 0 {
            0
        } else {
            TrapSignal::try_from(number)
                .ok()
                .and_then(|signal| i32::try_from(signal).ok())
                .ok_or_else(invalid)?
        }
    } else {
        TrapSignal::try_from(text)
            .ok()
            .filter(|signal| matches!(signal, TrapSignal::Signal(_)))
            .and_then(|signal| i32::try_from(signal).ok())
            .ok_or_else(invalid)?
    };
    u8::try_from(number).map_err(|_| invalid())
}

/// A signal's name as `timeout -v` prints it: `TERM`, or its number when it has none.
#[cfg(target_arch = "wasm32")]
fn signal_name(signal: u8) -> String {
    brush_core::traps::TrapSignal::try_from(i32::from(signal))
        .ok()
        .filter(|signal| matches!(signal, brush_core::traps::TrapSignal::Signal(_)))
        .map_or_else(
            || signal.to_string(),
            |signal| signal.as_str().trim_start_matches("SIG").to_owned(),
        )
}

impl SimpleCommand for Timeout {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Timeout::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!(
                "{name}: {name} [OPTION]... DURATION COMMAND [ARG]...\n"
            )),
            ContentType::DetailedHelp => Ok(HELP.to_owned()),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    /// Only `--help`, `--version` and errors: a command runs only where the shell runs
    /// processes it can signal (see `timeout_driver`).
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        use std::io::Write;
        let argv: Vec<String> = args.skip(1).map(|arg| arg.as_ref().to_owned()).collect();
        match plan(&argv) {
            Ok(Plan::Print(text)) => {
                let _ = context.stdout().write_all(text.as_bytes());
                Ok(ExecutionResult::success())
            }
            Ok(Plan::Run { .. }) => {
                let _ = context
                    .stderr()
                    .write_all(b"timeout: running a command is unsupported here\n");
                Ok(ExecutionResult::new(FAILED))
            }
            Err((message, code)) => {
                let _ = context.stderr().write_all(message.as_bytes());
                Ok(ExecutionResult::new(code))
            }
        }
    }
}

/// Runs `timeout`: COMMAND as a child process of this shell, raced against the timers that
/// signal it. The timers are the same in every run that reaches them: DURATION's, then
/// `--kill-after`'s once the first has fired.
#[cfg(target_arch = "wasm32")]
fn timeout_driver<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        use brush_core::execution::process;
        use futures::io::AsyncWriteExt;
        let argv: Vec<String> = args.iter().skip(1).map(ToString::to_string).collect();
        let (duration, signal, kill_after, preserve_status, verbose, command) = match plan(&argv) {
            Ok(Plan::Print(text)) => {
                context
                    .stdout()
                    .async_io()
                    .write_all(text.as_bytes())
                    .await?;
                return Ok(ExecutionResult::success());
            }
            Ok(Plan::Run {
                duration,
                signal,
                kill_after,
                preserve_status,
                verbose,
                command,
            }) => (
                duration,
                signal,
                kill_after,
                preserve_status,
                verbose,
                command,
            ),
            Err((message, code)) => {
                context
                    .stderr()
                    .async_io()
                    .write_all(message.as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(code));
            }
        };
        let name = &command[0];
        let cwd = context.shell.working_dir().to_path_buf();
        match super::xargs::lookup(context.shell, name, &cwd) {
            super::xargs::Lookup::Found => {}
            lookup => {
                let (reason, code) = match lookup {
                    super::xargs::Lookup::Denied => ("Permission denied", 126),
                    _ => ("No such file or directory", 127),
                };
                let message =
                    format!("timeout: failed to run command \u{2018}{name}\u{2019}: {reason}\n");
                context
                    .stderr()
                    .async_io()
                    .write_all(message.as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(code));
            }
        }
        let table = context.shell.processes().clone();
        let services = context.shell.execution_services();
        let pid = table.allocate(context.shell.own_pid(), String::new());
        let mut stderr = context.stderr();
        let timed_out = std::cell::Cell::new(false);
        let watchdog = async {
            // Zero disables the limit, as does one too long to ever pass (`inf`).
            if duration.is_zero() || duration == std::time::Duration::MAX {
                return std::future::pending::<()>().await;
            }
            (services.sleep)(duration).await;
            timed_out.set(true);
            if verbose {
                let note = format!(
                    "timeout: sending signal {} to command \u{2018}{name}\u{2019}\n",
                    signal_name(signal)
                );
                let _ = stderr.async_io().write_all(note.as_bytes()).await;
            }
            if signal != 0 {
                process::signal_process_group(&table, pid, signal);
            }
            if kill_after.is_zero() {
                return std::future::pending::<()>().await;
            }
            (services.sleep)(kill_after).await;
            if verbose {
                let note =
                    format!("timeout: sending signal KILL to command \u{2018}{name}\u{2019}\n");
                let _ = stderr.async_io().write_all(note.as_bytes()).await;
            }
            process::signal_process_group(&table, pid, KILL);
            std::future::pending::<()>().await
        };
        let line = super::xargs::command_line(&command);
        let params = context.params.clone();
        let child = super::xargs::run_child_as(
            context.shell,
            &params,
            None,
            line,
            None,
            None,
            false,
            Some(pid),
        );
        let result =
            match futures::future::select(std::pin::pin!(watchdog), std::pin::pin!(child)).await {
                futures::future::Either::Left(((), _)) => unreachable!("the watchdog never ends"),
                futures::future::Either::Right((result, _)) => result?,
            };
        // As GNU's: a command a signal ended ends timeout with that signal too (bash then reports
        // it), unless the command timed out, when the status is 124 (or, with
        // `--preserve-status`, the command's own); KILL at the limit reaches timeout itself.
        let status = u8::from(result.exit_code);
        Ok(match result.terminating_signal {
            Some(KILL) if timed_out.get() => ExecutionResult::terminated_by_signal(KILL),
            Some(signal) if !timed_out.get() => ExecutionResult::terminated_by_signal(signal),
            _ if timed_out.get() && !preserve_status => ExecutionResult::new(TIMED_OUT),
            _ => ExecutionResult::new(status),
        })
    })
}

/// The `timeout` registration: on wasm32 the driver runs the command; elsewhere only `--help`,
/// `--version` and errors are answered.
pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    #[allow(unused_mut, reason = "only wasm32 replaces the execute function")]
    let mut registration = brush_core::builtins::simple_builtin::<Timeout, SE>();
    #[cfg(target_arch = "wasm32")]
    {
        registration.execute_func = timeout_utility::<SE>;
    }
    vec![(Timeout::NAME.into(), registration)]
}

#[cfg(target_arch = "wasm32")]
fn timeout_utility<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    super::streaming::utility(context, args, timeout_driver::<SE>)
}

/// The `timeout` manifest.
pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(Timeout::NAME, Timeout::SYNOPSIS)]
}

#[cfg(test)]
mod tests {
    use super::{Plan, plan};
    use std::time::Duration;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(ToOwned::to_owned).collect()
    }

    #[test]
    fn options_and_operands_parse_as_gnu_timeout_does() {
        let Ok(Plan::Run {
            duration,
            signal,
            kill_after,
            preserve_status,
            verbose,
            command,
        }) = plan(&args("-vk 2m --sig=hup --pres 1.5 sleep -5"))
        else {
            panic!("not a run");
        };
        assert_eq!(duration, Duration::from_millis(1_500));
        assert_eq!((signal, kill_after), (1, Duration::from_secs(120)));
        assert!(preserve_status && verbose);
        assert_eq!(command, args("sleep -5"));
        assert!(matches!(plan(&args("--help")), Ok(Plan::Print(_))));
        assert!(matches!(
            plan(&args("-s 0 inf true")),
            Ok(Plan::Run { signal: 0, .. })
        ));
    }

    #[test]
    fn bad_arguments_are_timeouts_own_failure() {
        for (line, message) in [
            ("", ""),
            ("5", ""),
            (
                "x sleep 1",
                "timeout: invalid time interval \u{2018}x\u{2019}\n",
            ),
            ("-1 true", "timeout: invalid option -- '1'\n"),
            (
                "-s FOO 1 true",
                "timeout: \u{2018}FOO\u{2019}: invalid signal\n",
            ),
            (
                "-s 99 1 true",
                "timeout: \u{2018}99\u{2019}: invalid signal\n",
            ),
            ("--bogus 1 true", "timeout: unrecognized option '--bogus'\n"),
            ("-k", "timeout: option requires an argument -- 'k'\n"),
        ] {
            let Err((text, code)) = plan(&args(line)) else {
                panic!("{line:?} parsed");
            };
            assert_eq!(code, 125, "{line:?}");
            assert_eq!(
                text,
                format!("{message}Try 'timeout --help' for more information.\n"),
                "{line:?}"
            );
        }
    }
}
