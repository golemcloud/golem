//! `sh` and `bash`: run a script string through this same shell.
//!
//! `find -exec sh -c '…' _ {} \;` and `xargs bash -c '…'` hand work to a nested shell. There is
//! no second shell program here, so the script runs as an isolated child command
//! ([`super::xargs::run_child`]): a copy of this shell, like a subshell, so `cd`, assignments and
//! `exit` stay inside it. Like `eval`, the script must pass the finite-profile checks before any
//! of it runs.
//!
//! `sh -c SCRIPT NAME ARGS…` sets `$0` to NAME, as bash does; with no NAME, `$0` stays this
//! shell's own name.
//!
//! `bash run …` is not a shell invocation but the bound `bash` tool's own command, so it goes to
//! that tool (see `Session::register_commands`). Real bash would read a script file named `run`,
//! which is refused here anyway.

use brush_core::builtins::{
    BoxFuture, ContentOptions, ContentType, ExecutionBoundary, Registration,
};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::DefaultShellExtensions;
use brush_core::namedoptions::{self, ShellOptionKind};
use brush_core::openfiles::OpenFiles;
use brush_core::{CommandArg, Error, ExecutionResult};
use futures::io::AsyncWriteExt;

use super::xargs::{run_child_with, shell_quote};
use crate::manifest::Manifest;

/// Command names whose bound tool, if any, is reached through this command rather than
/// shadowed by it.
pub(crate) const FORWARDS_TO_TOOL: &[&str] = &["bash"];

/// Single-letter options that are `set` options, applied in the child before the script.
const SET_OPTIONS: &str = "aCefhnuvx";

const HELP: &str = "\
Usage: sh [OPTION]... -c SCRIPT [NAME [ARG]...]
       sh [OPTION]... [-s] [ARG]...
Run SCRIPT, or a script read from standard input, through this shell.

  -c              read the script from the SCRIPT operand; NAME becomes $0 and the
                    remaining ARGs become $1, $2, ...
  -s              read the script from standard input; ARGs become $1, $2, ...
  -a -C -e -f -h -n -u -v -x
                  set the matching shell option first (+ turns it off)
  -o NAME         set -o NAME first (+o turns it off)
  -O NAME         shopt -s NAME first (+O turns it off)
  -l, --login     a login shell (shopt login_shell); there are no profile files to read
  --posix         set -o posix first
  --noprofile, --norc, --noediting, --rcfile FILE, --init-file FILE
                  accepted: a shell that is not interactive reads no startup file
  -r, --restricted
                  refused: restricted shells are not supported

The script runs as an isolated child of this shell: cd, assignments and exit stay
inside it. -c SCRIPT NAME sets $0 to NAME, as bash does; with no NAME, $0 is this
shell's own name. Script files and interactive shells are not supported.
`bash run ...` invokes the bound bash tool.
";

/// Where the script comes from.
enum Script {
    Operand(String),
    Stdin,
}

struct Invocation {
    script: Script,
    /// `shopt` commands the child runs before the script.
    setup: Vec<String>,
    /// Options for the `set` command the child runs last before the script (`-e`, `-o`,
    /// `pipefail`, ...), which also sets the positional parameters.
    set_options: Vec<String>,
    /// `$0`: the operand after `-c SCRIPT`, else the name the shell was invoked by.
    name: String,
    /// Whether `-c SCRIPT` had a NAME operand after it.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(dead_code, reason = "env runs its command only on wasm32")
    )]
    named: bool,
    /// `$1` and later.
    args: Vec<String>,
    /// A login shell (`-l`, `--login`).
    login: bool,
}

enum Parsed {
    Run(Invocation),
    Tool,
    Help,
    Version,
}

/// A usage error or refusal: the complete stderr text and exit status.
#[derive(Debug, PartialEq)]
struct Failure {
    text: String,
    code: u8,
}

/// An option bash does not know: the message, then bash's own usage summary.
fn usage(name: &str, message: &str) -> Failure {
    Failure {
        text: format!(
            "{name}: {message}\n\
             Usage:\t{name} [GNU long option] [option] ...\n\
             \t{name} [GNU long option] [option] script-file ...\n\
             GNU long options:\n\
             \t--debug\n\t--debugger\n\t--dump-po-strings\n\t--dump-strings\n\t--help\n\
             \t--init-file\n\t--login\n\t--noediting\n\t--noprofile\n\t--norc\n\t--posix\n\
             \t--pretty-print\n\t--rcfile\n\t--restricted\n\t--verbose\n\t--version\n\
             Shell options:\n\
             \t-ilrsD or -c command or -O shopt_option\t\t(invocation only)\n\
             \t-abefhkmnptuvxBCEHPT or -o option\n"
        ),
        code: 2,
    }
}

/// `--version`, as bash words it, for the version this shell reports in `BASH_VERSION`.
fn version_text(version: &str, machine: &str) -> String {
    format!(
        "GNU bash, version {version} ({machine})\n\
         Copyright (C) 2022 Free Software Foundation, Inc.\n\
         License GPLv3+: GNU GPL version 3 or later <http://gnu.org/licenses/gpl.html>\n\
         \n\
         This is free software; you are free to change and redistribute it.\n\
         There is NO WARRANTY, to the extent permitted by law.\n"
    )
}

/// An option name `-o` or `-O` does not know, reported as bash reports it before it runs
/// anything.
fn invalid_option_name(name: &str, letter: char, option: &str) -> Failure {
    let text = if letter == 'o' {
        format!("{name}: line 0: {name}: {option}: invalid option name\n")
    } else {
        format!("{name}: line 0: {option}: invalid shell option name\n")
    };
    Failure { text, code: 2 }
}

/// An option given without its argument: bash says so and prints no usage hint.
fn missing_argument(name: &str, option: &str) -> Failure {
    Failure {
        text: format!("{name}: {option}: option requires an argument\n"),
        code: 2,
    }
}

fn unsupported(name: &str, feature: &str) -> Failure {
    Failure {
        text: format!("{name}: {feature} is unsupported in bash-tool\n"),
        code: 2,
    }
}

fn parse(name: &str, args: &[String]) -> Result<Parsed, Failure> {
    if FORWARDS_TO_TOOL.contains(&name) && args.first().is_some_and(|arg| arg == "run") {
        return Ok(Parsed::Tool);
    }
    let mut command = false;
    let mut stdin = false;
    let mut login = false;
    let mut setup = Vec::new();
    let mut set_options = Vec::new();
    // Bash reads long options only before the first single-letter one.
    let mut short_seen = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--" | "-" => {
                index += 1;
                break;
            }
            "--help" if !short_seen => return Ok(Parsed::Help),
            "--version" if !short_seen => return Ok(Parsed::Version),
            // A login shell reads profile files; there are none here.
            "--login" if !short_seen => login = true,
            "--posix" if !short_seen => {
                set_options.push("-o".to_owned());
                set_options.push("posix".to_owned());
            }
            "--noprofile" | "--norc" | "--noediting" if !short_seen => {}
            // Only an interactive shell reads the file, and this one is not: bash ignores it.
            "--rcfile" | "--init-file" if !short_seen => {
                index += 1;
                if args.get(index).is_none() {
                    let option = arg.trim_start_matches('-');
                    return Err(missing_argument(name, option));
                }
            }
            "--restricted" if !short_seen => {
                return Err(unsupported(name, "a restricted shell (--restricted)"));
            }
            long if long.starts_with("--") && !short_seen => {
                return Err(usage(name, &format!("{long}: invalid option")));
            }
            flags if flags.len() > 1 && (flags.starts_with('-') || flags.starts_with('+')) => {
                short_seen = true;
                let sign = &flags[..1];
                for letter in flags[1..].chars() {
                    match letter {
                        'c' if sign == "-" => command = true,
                        's' if sign == "-" => stdin = true,
                        // A login shell reads profile files; there are none here.
                        'l' => login = sign == "-",
                        'r' => return Err(unsupported(name, "a restricted shell (-r)")),
                        'i' => return Err(unsupported(name, "an interactive shell (-i)")),
                        'o' | 'O' => {
                            index += 1;
                            let Some(option) = args.get(index) else {
                                return Err(missing_argument(name, &format!("{sign}{letter}")));
                            };
                            let kind = if letter == 'o' {
                                ShellOptionKind::SetO
                            } else {
                                ShellOptionKind::Shopt
                            };
                            if namedoptions::options(kind).get(option).is_none() {
                                return Err(invalid_option_name(name, letter, option));
                            }
                            if letter == 'o' {
                                set_options.push(format!("{sign}o"));
                                set_options.push(shell_quote(option));
                            } else {
                                let action = if sign == "-" { "-s" } else { "-u" };
                                setup.push(format!("shopt {action} {}", shell_quote(option)));
                            }
                        }
                        letter if SET_OPTIONS.contains(letter) => {
                            set_options.push(format!("{sign}{letter}"));
                        }
                        other => {
                            return Err(usage(name, &format!("{sign}{other}: invalid option")));
                        }
                    }
                }
            }
            _ => break,
        }
        index += 1;
    }
    let operands = &args[index..];
    if command {
        let Some((script, rest)) = operands.split_first() else {
            return Err(missing_argument(name, "-c"));
        };
        return Ok(Parsed::Run(Invocation {
            script: Script::Operand(script.clone()),
            setup,
            set_options,
            name: rest.first().map_or_else(|| name.to_owned(), Clone::clone),
            named: !rest.is_empty(),
            args: rest.iter().skip(1).cloned().collect(),
            login,
        }));
    }
    if !stdin && let Some(file) = operands.first() {
        return Err(Failure {
            text: format!(
                "{name}: {file}: running a script file is unsupported in bash-tool; \
                 use `source {file}` or `{name} -c`\n"
            ),
            code: 2,
        });
    }
    Ok(Parsed::Run(Invocation {
        script: Script::Stdin,
        setup,
        set_options,
        name: name.to_owned(),
        named: false,
        args: operands.to_vec(),
        login,
    }))
}

/// The arguments that give `sh` or `bash` (`name`) the argv[0] `argv0`, as `env -a` would: with `-c SCRIPT` and no NAME after it, bash takes argv[0] as `$0`, so it becomes that
/// NAME; with a NAME, argv[0] changes nothing. `None` when the script would come from standard
/// input, whose `$0` is argv[0] itself, which this shell has no way to give it.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "env runs its command only on wasm32")
)]
pub(crate) fn with_argv0(name: &str, args: &[String], argv0: &str) -> Option<Vec<String>> {
    match parse(name, args) {
        Ok(Parsed::Run(Invocation {
            script: Script::Operand(_),
            named: false,
            ..
        })) => {
            let mut args = args.to_vec();
            args.push(argv0.to_owned());
            Some(args)
        }
        Ok(Parsed::Run(Invocation {
            script: Script::Stdin,
            ..
        })) => None,
        _ => Some(args.to_vec()),
    }
}

/// What runs before the script in the child: the `shopt` options, `SHLVL`, and one `set` of the
/// options and positional parameters. `-x` and `-v` take effect only with that last command, so
/// none of this is traced or echoed, as bash applies its options before it reads the script.
fn child_prologue(invocation: &Invocation, shell_level: u32) -> String {
    let mut line = String::new();
    for command in &invocation.setup {
        line.push_str(command);
        line.push('\n');
    }
    line.push_str(&format!("export SHLVL={shell_level}\nset"));
    for option in &invocation.set_options {
        line.push(' ');
        line.push_str(option);
    }
    line.push_str(" --");
    for arg in &invocation.args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    line
}

/// `SHLVL` in a new shell: one more than its caller's exported value, as bash counts it (0 when
/// that is missing or not a number; 1 again at 1000, which bash warns of: the second value).
fn child_shell_level(shell: &brush_core::Shell<DefaultShellExtensions>) -> (u32, Option<i64>) {
    let caller = shell
        .env()
        .get("SHLVL")
        .filter(|(_, var)| var.is_exported())
        .and_then(|(_, var)| var.value().to_cow_str(shell).trim().parse::<i64>().ok())
        .unwrap_or(0);
    match caller.saturating_add(1) {
        level if level < 0 => (0, None),
        level if level >= 1000 => (1, Some(level)),
        level => (u32::try_from(level).unwrap_or(1), None),
    }
}

async fn read_stdin(context: &ExecutionContext<'_>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    #[cfg(target_arch = "wasm32")]
    {
        use futures::io::AsyncReadExt;
        super::streaming::input(context)
            .async_io()
            .read_to_end(&mut bytes)
            .await?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::io::Read::read_to_end(&mut super::coreutils::tool_stdin(context), &mut bytes)?;
    Ok(bytes)
}

fn execute(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let name = context.command_name.clone();
        let words: Vec<String> = args.iter().skip(1).map(ToString::to_string).collect();
        let mut stderr = context.stderr();
        let invocation = match parse(&name, &words) {
            Ok(Parsed::Run(invocation)) => invocation,
            Ok(Parsed::Tool) => return crate::commands::forward(context, args).await,
            Ok(Parsed::Help) => {
                context
                    .stdout()
                    .async_io()
                    .write_all(HELP.replace("sh ", &format!("{name} ")).as_bytes())
                    .await?;
                return Ok(ExecutionResult::success());
            }
            Ok(Parsed::Version) => {
                let variable = |variable| {
                    context
                        .shell
                        .env_str(variable)
                        .map(|value| value.into_owned())
                        .unwrap_or_default()
                };
                let version = version_text(&variable("BASH_VERSION"), &variable("MACHTYPE"));
                context
                    .stdout()
                    .async_io()
                    .write_all(version.as_bytes())
                    .await?;
                return Ok(ExecutionResult::success());
            }
            Err(failure) => {
                stderr.async_io().write_all(failure.text.as_bytes()).await?;
                return Ok(ExecutionResult::new(failure.code));
            }
        };
        let mut params = context.params.clone();
        let script = match &invocation.script {
            Script::Operand(script) => script.clone(),
            Script::Stdin => {
                let bytes = read_stdin(&context).await?;
                // The script consumed standard input; the child reads end-of-file.
                params.set_fd(
                    OpenFiles::STDIN_FD,
                    brush_core::openfiles::from_bytes(Vec::new()),
                );
                // Every byte of the script is kept, UTF-8 or not, as bash reads it.
                super::shell_bytes::decode_vec(bytes)
            }
        };
        if let Err(error) = crate::session::validate_script(&script, "-c") {
            let message = crate::session::diagnostic(&name, &error);
            stderr.async_io().write_all(message.as_bytes()).await?;
            return Ok(ExecutionResult::new(2));
        }
        let (shell_level, too_high) = child_shell_level(context.shell);
        if let Some(level) = too_high {
            let warning =
                format!("{name}: warning: shell level ({level}) too high, resetting to 1\n");
            stderr.async_io().write_all(warning.as_bytes()).await?;
        }
        let prologue = child_prologue(&invocation, shell_level);
        let result = run_child_with(
            &mut *context.shell,
            &params,
            Some(prologue),
            script,
            None,
            Some(&invocation.name),
            true,
            None,
            invocation.login,
        )
        .await?;
        // The child's `exit` ends the child only: keep its status and signal, not its control flow.
        let mut outcome = ExecutionResult::new(u8::from(result.exit_code));
        outcome.terminating_signal = result.terminating_signal;
        Ok(outcome)
    })
}

#[cfg(target_arch = "wasm32")]
fn execute_utility(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    super::streaming::utility(context, args, execute)
}

fn content(name: &str, content_type: ContentType, _: &ContentOptions) -> Result<String, Error> {
    match content_type {
        ContentType::ShortDescription => Ok(format!(
            "{name} - run a script through this shell as an isolated child\n"
        )),
        ContentType::ShortUsage => Ok(format!(
            "{name}: {name} [OPTION]... -c SCRIPT [NAME [ARG]...]\n"
        )),
        ContentType::DetailedHelp => Ok(HELP.replace("sh ", &format!("{name} "))),
        ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
    }
}

pub(crate) fn registration() -> Registration<DefaultShellExtensions> {
    let registration = Registration {
        execute_func: execute,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
        execution_boundary: ExecutionBoundary::Command,
    };
    #[cfg(target_arch = "wasm32")]
    let registration = Registration {
        execute_func: execute_utility,
        ..registration
    };
    registration
}

pub(crate) fn manifests() -> Vec<Manifest> {
    let help = "sh|bash [-aCefhnuvx] [-o NAME] [-O NAME] -c SCRIPT [NAME [ARG...]], or a \
                script on standard input — runs the script through this shell as an isolated \
                child (cd, assignments and exit stay inside it); NAME sets $0, as bash does, \
                else $0 is this shell's name. The script passes the same checks as a top-level \
                script. Script files and interactive shells are refused. `bash run ...` invokes \
                the bound bash tool.";
    vec![
        Manifest::builtin("sh", "run a script through this shell").with_help(help),
        Manifest::builtin("bash", "run a script through this shell").with_help(help),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, args: &[&str]) -> Result<Parsed, Failure> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        parse(name, &args)
    }

    fn invocation(name: &str, args: &[&str]) -> Invocation {
        match run(name, args) {
            Ok(Parsed::Run(invocation)) => invocation,
            _ => panic!("expected a run for {args:?}"),
        }
    }

    #[test]
    fn command_string_skips_the_name_operand() {
        let parsed = invocation("sh", &["-c", "echo $1 $2", "_", "a b", "c"]);
        assert!(matches!(&parsed.script, Script::Operand(script) if script == "echo $1 $2"));
        assert_eq!(parsed.name, "_");
        assert_eq!(parsed.args, ["a b", "c"]);
        assert_eq!(child_prologue(&parsed, 2), "export SHLVL=2\nset -- 'a b' c");
        assert_eq!(invocation("sh", &["-c", "echo $0"]).name, "sh");
    }

    #[test]
    fn option_clusters_become_setup_commands() {
        let parsed = invocation("bash", &["-euxc", "true"]);
        assert_eq!(parsed.set_options, ["-e", "-u", "-x"]);
        assert_eq!(
            child_prologue(&parsed, 1),
            "export SHLVL=1\nset -e -u -x --"
        );
        let parsed = invocation("bash", &["-o", "pipefail", "+O", "extglob", "-lc", "true"]);
        assert_eq!(parsed.set_options, ["-o", "pipefail"]);
        assert_eq!(parsed.setup, ["shopt -u extglob"]);
        let parsed = invocation("sh", &["--login", "--norc", "-c", "true"]);
        assert!(parsed.setup.is_empty() && parsed.set_options.is_empty() && parsed.login);
    }

    #[test]
    fn stdin_scripts_take_operands_as_arguments() {
        let parsed = invocation("sh", &["-s", "a", "b"]);
        assert!(matches!(parsed.script, Script::Stdin));
        assert_eq!(parsed.args, ["a", "b"]);
        assert!(matches!(invocation("sh", &[]).script, Script::Stdin));
    }

    #[test]
    fn bash_run_goes_to_the_bound_tool() {
        assert!(matches!(run("bash", &["run", "x"]), Ok(Parsed::Tool)));
        // `sh` has no bound tool of its own; `run` is a script file there.
        assert!(run("sh", &["run"]).is_err());
    }

    #[test]
    fn refusals_and_usage_errors() {
        let file = run("sh", &["script.sh"]).err().unwrap();
        assert_eq!(file.code, 2);
        assert!(
            file.text.contains("unsupported in bash-tool"),
            "{}",
            file.text
        );
        assert_eq!(run("bash", &["-i"]).err().unwrap().code, 2);
        assert_eq!(
            run("sh", &["-c"]).err().unwrap().text,
            "sh: -c: option requires an argument\n"
        );
        assert!(
            run("sh", &["-Q"])
                .err()
                .unwrap()
                .text
                .starts_with("sh: -Q: invalid option\nUsage:\tsh [GNU long option] [option] ...\n")
        );
        // Long options come before single-letter ones.
        assert!(
            run("bash", &["-O", "extglob", "--rcfile", "x", "-c", "true"])
                .err()
                .unwrap()
                .text
                .starts_with("bash: --: invalid option\n")
        );
        assert_eq!(
            run("bash", &["--rcfile"]).err().unwrap().text,
            "bash: rcfile: option requires an argument\n"
        );
        assert_eq!(run("bash", &["-r", "-c", "true"]).err().unwrap().code, 2);
        assert!(
            invocation("bash", &["--rcfile", "/x", "-c", "true"])
                .set_options
                .is_empty()
        );
        assert!(invocation("bash", &["-l", "-c", "true"]).login);
        assert_eq!(
            invocation("sh", &["--posix", "-c", "true"]).set_options,
            ["-o", "posix"]
        );
        assert!(matches!(run("sh", &["--help"]), Ok(Parsed::Help)));
        assert_eq!(
            run("bash", &["-o", "bogus", "-c", "true"]).err().unwrap(),
            Failure {
                text: "bash: line 0: bash: bogus: invalid option name\n".to_owned(),
                code: 2
            }
        );
        assert_eq!(
            run("bash", &["+O", "bogus", "-c", "true"])
                .err()
                .unwrap()
                .text,
            "bash: line 0: bogus: invalid shell option name\n"
        );
    }
}
