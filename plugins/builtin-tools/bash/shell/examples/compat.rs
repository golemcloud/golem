//! A bash-like command line over the bash tool's shell, so Brush's compatibility suite can run
//! against it under Wasmtime (see `conformance/compat/`).
//!
//! `compat [--status-file PATH] [--cwd DIR] [BRUSH FLAGS] [-euxfav] [-o OPT] [-O SHOPT] [--posix]
//!        [-c SCRIPT [NAME [ARG...]] | FILE [ARG...]]`
//!
//! Without `-c` or FILE the script is read from stdin. Brush's own startup flags (`--norc` and
//! friends) are accepted and ignored. Options, positional parameters and the working directory
//! are applied by separate evaluations before the script runs, so the script's line numbers are
//! its own.
use std::io::{Read, Write};

use bash_shell::session::Session;

const IGNORED: &[&str] = &[
    "--norc",
    "--noprofile",
    "--no-config",
    "--disable-bracketed-paste",
    "--disable-color",
    "--login",
    "-l",
];

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Default)]
struct Invocation {
    status_file: Option<String>,
    cwd: Option<String>,
    prologue: Vec<String>,
    script: Option<String>,
    file: Option<String>,
    /// `$0`, when `-c SCRIPT NAME` gives one.
    name: Option<String>,
    args: Vec<String>,
}

fn parse(mut argv: impl Iterator<Item = String>) -> Result<Invocation, String> {
    let mut inv = Invocation::default();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--status-file" => inv.status_file = argv.next(),
            "--cwd" => inv.cwd = argv.next(),
            "--posix" => inv.prologue.push("set -o posix".into()),
            "-c" => {
                inv.script = Some(argv.next().ok_or("-c: option requires an argument")?);
                // NAME becomes $0; the rest are $@.
                inv.name = argv.next();
                inv.args = argv.collect();
                break;
            }
            "-o" | "+o" => {
                let name = argv.next().ok_or("-o: option requires an argument")?;
                inv.prologue.push(format!("set {arg} {}", quote(&name)));
            }
            "-O" | "+O" => {
                let name = argv.next().ok_or("-O: option requires an argument")?;
                let mode = if arg == "-O" { "-s" } else { "-u" };
                inv.prologue.push(format!("shopt {mode} {}", quote(&name)));
            }
            "-i" => return Err("interactive shells are unsupported".into()),
            "-s" | "--" => {
                inv.args = argv.collect();
                break;
            }
            _ if IGNORED.contains(&arg.as_str()) || arg.starts_with("--input-backend") => {}
            _ if (arg.starts_with('-') || arg.starts_with('+')) && arg.len() > 1 => {
                inv.prologue.push(format!("set {arg}"));
            }
            _ => {
                inv.file = Some(arg);
                inv.args = argv.collect();
                break;
            }
        }
    }
    Ok(inv)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let inv = match parse(std::env::args().skip(1)) {
        Ok(inv) => inv,
        Err(message) => {
            eprintln!("bash: {message}");
            std::process::exit(2);
        }
    };
    let script = match (&inv.script, &inv.file) {
        (Some(script), _) => script.clone(),
        (None, Some(file)) => match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("bash: {file}: {error}");
                write_status(&inv, 127)?;
                std::process::exit(127);
            }
        },
        (None, None) => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        }
    };
    let mut prologue = inv.prologue.clone();
    if !inv.args.is_empty() {
        let args: Vec<String> = inv.args.iter().map(|a| quote(a)).collect();
        prologue.push(format!("set -- {}", args.join(" ")));
    }

    // Natively, command substitutions use Tokio's pipes, which need its IO driver.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    #[cfg(target_arch = "wasm32")]
    let result = runtime.block_on(tokio::task::LocalSet::new().run_until(async {
        let mut shell =
            Session::new_with_execution_services(bash_shell::ExecutionServices::default()).await?;
        prepare(&mut shell, &inv, &prologue).await;
        Ok::<_, brush_core::Error>(shell.run(&script).await)
    }))?;
    #[cfg(not(target_arch = "wasm32"))]
    let result = runtime.block_on(async {
        let mut shell = Session::new().await?;
        prepare(&mut shell, &inv, &prologue).await;
        Ok::<_, brush_core::Error>(shell.run(&script).await)
    })?;
    std::io::stdout().write_all(&result.stdout)?;
    std::io::stderr().write_all(&result.stderr)?;
    write_status(&inv, result.exit_code)?;
    std::process::exit(i32::from(result.exit_code));
}

async fn prepare(shell: &mut Session, inv: &Invocation, prologue: &[String]) {
    if let Some(name) = &inv.name {
        shell.set_shell_name(name);
    }
    // Only `-c` runs a command string; bash reads standard input or a file a command at a time.
    if inv.script.is_none() {
        shell.read_scripts_as_input();
    }
    if let Some(cwd) = &inv.cwd
        && let Err(error) = shell.set_working_dir(cwd)
    {
        eprintln!("compat: {error}");
    }
    for line in prologue {
        let result = shell.run(line).await;
        if result.exit_code != 0 {
            let _ = std::io::stderr().write_all(&result.stderr);
        }
    }
}

fn write_status(inv: &Invocation, status: u8) -> std::io::Result<()> {
    if let Some(path) = &inv.status_file {
        std::fs::write(path, status.to_string())?;
    }
    Ok(())
}
