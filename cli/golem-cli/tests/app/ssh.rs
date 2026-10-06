// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::builtin_bash::{self, OWNER, invoke};
use super::{InteractiveSession, RawOutput, TestContext};
use golem_cli::fs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use test_r::{test, timeout};

const BASH_ONLY: &str = r#"BashOnlyOwner("isolated")"#;
const DENIED_FILES: &str = r#"DeniedFilesOwner("denied")"#;
const MARKER: &str = "\u{276f}";

// Runs `golem ssh` with piped input: the commands are read from it and stdout carries only the
// scripts' output.
// The sessions here are a person's, whatever runs the tests: in an AI agent's environment every
// session would otherwise be a plain one.
async fn context() -> TestContext {
    let mut ctx = builtin_bash::context().await;
    ctx.add_env_var("GOLEM_CLI_AGENT_HINTS", "0");
    ctx
}

async fn ssh(ctx: &TestContext, args: &[&str], input: &str) -> RawOutput {
    let mut command = vec!["ssh"];
    command.extend(args);
    tokio::time::timeout(
        Duration::from_secs(180),
        ctx.cli_with_input(command, input.as_bytes()),
    )
    .await
    .expect("golem ssh did not finish")
}

fn status(output: &RawOutput) -> i32 {
    output
        .exit_code()
        .expect("golem ssh was killed by a signal")
}

fn assert_output(output: &RawOutput, status_code: i32, stdout: &str, stderr: &str) {
    assert_eq!(
        (status(output), output.stdout_text(), output.stderr_text()),
        (status_code, stdout.to_string(), stderr.to_string())
    );
}

fn assert_not_run(output: &RawOutput, diagnostic: &str) {
    assert_eq!(status(output), 255, "{}", output.stderr_text());
    assert_eq!(output.stdout_text(), "");
    assert!(
        output.stderr_text().contains(diagnostic),
        "{}",
        output.stderr_text()
    );
}

// A terminal answers the line editor's cursor-position query before each prompt; the PTY does
// not, so the test answers it.
fn answer_cursor_query(session: &mut dyn InteractiveSession) -> anyhow::Result<()> {
    session.expect_str("\x1b[6n")?;
    session.send("\x1b[1;1R")
}

// Types a line and presses Enter.
fn enter(session: &mut dyn InteractiveSession, line: &str) -> anyhow::Result<()> {
    session.send(&format!("{line}\r"))
}

// Runs the CLI synchronously with the test context's configuration, for use while an
// interactive session holds the test thread.
struct BlockingCli {
    path: PathBuf,
    config_dir: PathBuf,
    working_dir: PathBuf,
    env: HashMap<String, String>,
}

impl BlockingCli {
    fn new(ctx: &TestContext) -> Self {
        Self {
            path: ctx.golem_cli_path.clone(),
            config_dir: ctx.config_dir.path().to_path_buf(),
            working_dir: fs::absolute_lexical_path(&ctx.working_dir).unwrap(),
            env: ctx.env.clone(),
        }
    }

    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = std::process::Command::new(&self.path)
            .arg("--config-dir")
            .arg(&self.config_dir)
            .args(args)
            .env_remove("GOLEM_BUILTIN_LOCAL_URL")
            .envs(&self.env)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[test]
#[timeout("20 minutes")]
async fn ssh_validates_the_tool_before_running() {
    let ctx = context().await;

    // The default `bash` binding of an existing agent connects and runs.
    let connected = ssh(&ctx, &[OWNER, "-c", "printf ok"], "").await;
    assert_output(&connected, 0, "ok", "");

    // A missing agent is reported, not created: a second attempt fails the same way.
    for _ in 0..2 {
        let missing = ssh(&ctx, &[r#"BashOwner("missing")"#, "-c", "true"], "").await;
        assert_not_run(&missing, "not found");
    }

    // An owner without the binding is refused as such.
    let absent = ssh(&ctx, &[BASH_ONLY, "--tool", "fixture", "-c", "true"], "").await;
    assert_not_run(&absent, "`fixture` is not bound to");

    // A bound tool that does not offer bash's `run` is refused before anything is submitted.
    let incompatible = ssh(
        &ctx,
        &[
            OWNER,
            "--tool",
            "fixture",
            "-c",
            "mkdir -p /tmp/ssh && printf ran >/tmp/ssh/marker",
        ],
        "",
    )
    .await;
    assert_not_run(&incompatible, "is not a compatible bash tool");
    let marker = invoke(&ctx, OWNER, "", "test ! -e /tmp/ssh/marker").await;
    assert_eq!(marker.exit_code, 0, "{marker:?}");
}

#[test]
#[timeout("20 minutes")]
async fn ssh_runs_commands_in_order_and_idles_without_holding_the_owner() {
    let ctx = context().await;
    let other = BlockingCli::new(&ctx);

    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        session.expect_str("Connected to")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        enter(
            session,
            "mkdir -p /tmp/ssh-order && cd /tmp/ssh-order && echo first",
        )?;
        session.expect_str("first")?;
        answer_cursor_query(session)?;
        // The prompt shows the directory the command ended in.
        session.expect_str("/tmp/ssh-order")?;
        enter(session, "echo second; exit 7")?;
        session.expect_str("second")?;
        answer_cursor_query(session)?;
        // `$?` cannot reach the next command, so the prompt shows the status.
        session.expect_str("[7]")?;

        // Waiting at the prompt holds nothing open on the agent: other calls on it complete.
        let name = other.run(&["agent", "invoke", OWNER, "name"])?;
        anyhow::ensure!(name.contains("acceptance"), "{name}");
        let direct = other.run(&[
            "--format",
            "json",
            "tool",
            "invoke",
            "--agent",
            OWNER,
            "bash",
            "--",
            "run",
            "--",
            "printf idle",
        ])?;
        anyhow::ensure!(direct.contains("idle"), "{direct}");

        // Ctrl+C clears the line being typed; nothing is submitted.
        session.send("echo discarded")?;
        session.send("\u{3}")?;
        answer_cursor_query(session)?;
        enter(session, "pwd")?;
        session.expect_regex("\r\n/tmp/ssh-order\r\n")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // A terminal that does not answer the cursor-position query in time still gets plain line
    // input, and its late answer does not reach the next command.
    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(60)));
        session.expect_str(MARKER)?;
        session.send("\x1b[1;1R")?;
        enter(session, "pwd")?;
        session.expect_str("/\r\n")?;
        session.expect_str(&format!("/ {MARKER}"))?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn ssh_is_plain_for_an_ai_agent() {
    let mut ctx = context().await;
    // Colours are on, as for a person at this terminal, and an AI agent is driving it.
    ctx.env_mut().remove("NO_COLOR");
    ctx.add_env_var("GOLEM_CLI_AGENT_HINTS", "1");

    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(60)));
        // The line before it is the CLI's own, coloured as for any command. From the session's
        // first line on there is no escape sequence: no colour, and no question to the terminal,
        // which this one never answers.
        session.expect_str("Connected to")?;
        session.expect_regex(&format!("^[^\x1b]*{MARKER} "))?;
        // Slow enough for the running line, which is not drawn: the command's output and the
        // next prompt follow what was typed, with at most the terminal's own echo before them.
        enter(session, "sleep 2; echo plain")?;
        session.expect_regex(&format!(
            "^(sleep 2; echo plain\r\n)?plain\r\n[^\x1b\r]*{MARKER} "
        ))?;
        enter(session, "cd /nowhere")?;
        session.expect_regex(&format!("^[^\x1b]*\\[1\\] {MARKER} "))?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn ssh_carries_only_the_directory() {
    let ctx = context().await;

    let output = ssh(
        &ctx,
        &[OWNER],
        concat!(
            "mkdir -p /tmp/ssh-carry && cd /tmp/ssh-carry\n",
            "pwd\n",
            "X=kept; f() { echo fn; }; alias a='echo alias'; set -o noglob\n",
            "echo \"[${X-unset}]\"; type f >/dev/null 2>&1 || echo no-function; ",
            "alias a >/dev/null 2>&1 || echo no-alias; ",
            "case $- in *f*) echo noglob ;; *) echo globbing ;; esac\n",
        ),
    )
    .await;
    assert_output(
        &output,
        0,
        "/tmp/ssh-carry\n[unset]\nno-function\nno-alias\nglobbing\n",
        "",
    );
}

#[test]
#[timeout("20 minutes")]
async fn ssh_piped_input_stops_when_its_directory_is_refused() {
    let ctx = context().await;

    // The remembered directory is removed: the next command is refused before it runs. Piped
    // commands were written for that directory, so none of them runs anywhere else.
    let output = ssh(
        &ctx,
        &[OWNER],
        "mkdir -p /tmp/gone && cd /tmp/gone\nrmdir /tmp/gone\npwd\ntouch ran-elsewhere\npwd\n",
    )
    .await;
    assert_not_run(&output, "`invalid-cwd`");
    assert!(
        output
            .stderr_text()
            .contains("The directory cannot be used, so the commands after this one were not run."),
        "{}",
        output.stderr_text()
    );

    // The same when the directory asked for with `--cwd` is not there.
    let output = ssh(
        &ctx,
        &[OWNER, "--cwd", "/tmp/nowhere"],
        "pwd\ntouch ran-elsewhere\n",
    )
    .await;
    assert_not_run(&output, "`invalid-cwd`");
    let left = invoke(&ctx, OWNER, "/", "ls ran-elsewhere 2>/dev/null; echo done").await;
    assert_eq!(left.stdout, "done\n", "{left:?}");
}

#[test]
#[timeout("20 minutes")]
async fn ssh_piped_lines_run_as_whole_commands() {
    let ctx = context().await;

    // The lines of an `if`, a loop and a here-document are one command each, as bash reads
    // them. A comment line is not a command, and only a line that is `exit` ends the session.
    let output = ssh(
        &ctx,
        &[OWNER],
        concat!(
            "mkdir -p /tmp/ssh-whole && cd /tmp/ssh-whole && : > keep\n",
            "if false; then\n",
            "  rm -v keep\n",
            "fi\n",
            "cat > notes.txt <<EOF\n",
            "rm -v keep\n",
            "EOF\n",
            "for name in a b; do\n",
            "  echo \"item $name\"\n",
            "done\n",
            "ls\n",
            "cat notes.txt\n",
            "false\n",
            "# the status of the line before stands\n",
        ),
    )
    .await;
    assert_output(
        &output,
        1,
        "item a\nitem b\nkeep\nnotes.txt\nrm -v keep\n",
        "",
    );

    let output = ssh(&ctx, &[OWNER], "echo one\nexit 3 # stop here\necho two\n").await;
    assert_output(&output, 3, "one\n", "");
}

#[test]
#[timeout("20 minutes")]
async fn ssh_command_mode_passes_output_and_status_through() {
    let ctx = context().await;

    // Exactly the script's bytes and status; no CLI text on either stream.
    let output = ssh(
        &ctx,
        &[OWNER, "-c", "printf out; printf err >&2; exit 7"],
        "",
    )
    .await;
    assert_output(&output, 7, "out", "err");
    let output = ssh(&ctx, &[OWNER, "-c", "true"], "").await;
    assert_output(&output, 0, "", "");

    // A named tool error means nothing ran: status 255 and a diagnostic naming it.
    let output = ssh(&ctx, &[OWNER, "--cwd", "tmp", "-c", "pwd"], "").await;
    assert_not_run(&output, "`invalid-cwd`");

    // `--timeout` reaches the tool, which stops the script and returns 124.
    let output = ssh(
        &ctx,
        &[
            OWNER,
            "--timeout",
            "1",
            "-c",
            "echo start; while :; do x=1; done",
        ],
        "",
    )
    .await;
    assert_output(
        &output,
        124,
        "start\n",
        "bash: the call exceeded its 1 s time limit\n",
    );
}

#[test]
#[timeout("20 minutes")]
async fn ssh_scripts_reach_sibling_tools_and_shared_files() {
    let ctx = context().await;

    // The same output and status as a direct external invocation of the same script.
    for (owner, script) in [
        (OWNER, "fixture transfer --help"),
        (
            OWNER,
            "printf 'sibling-stream' | fixture transfer 'prefix:' | cat",
        ),
        (
            OWNER,
            "printf '%s' \"$(printf sub | fixture transfer prefix:)\"",
        ),
        (OWNER, "fixture fail"),
        (BASH_ONLY, "fixture fail"),
    ] {
        let through_ssh = ssh(&ctx, &[owner, "-c", script], "").await;
        let direct = invoke(&ctx, owner, "", script).await;
        assert_output(
            &through_ssh,
            i32::from(direct.exit_code),
            &direct.stdout,
            &direct.stderr,
        );
    }

    // Bash and the sibling see the same owner files, whichever way they are reached.
    let written = ssh(
        &ctx,
        &[
            OWNER,
            "-c",
            "mkdir -p /tmp/ssh-shared && fixture write /tmp/ssh-shared/one via-sibling >/dev/null",
        ],
        "",
    )
    .await;
    assert_output(&written, 0, "", "");
    let read = invoke(&ctx, OWNER, "", "cat /tmp/ssh-shared/one").await;
    assert_eq!(read.stdout, "via-sibling", "{read:?}");
    let written = invoke(&ctx, OWNER, "", "printf via-bash >/tmp/ssh-shared/two").await;
    assert_eq!(written.exit_code, 0, "{written:?}");
    let read = ssh(&ctx, &[OWNER, "-c", "fixture read /tmp/ssh-shared/two"], "").await;
    assert_eq!(status(&read), 0, "{}", read.stderr_text());
    assert!(
        read.stdout_text().contains("via-bash"),
        "{}",
        read.stdout_text()
    );
}

#[test]
#[timeout("20 minutes")]
async fn ssh_reports_denied_operations() {
    let ctx = context().await;
    let script = concat!(
        "mkdir -p /tmp/guarded && printf owner-data >/tmp/guarded/file && ",
        "fixture write /tmp/guarded/file forbidden",
    );

    // With `-c`, the denial is reported with a nonzero status and the process exits.
    let output = ssh(&ctx, &[DENIED_FILES, "-c", script], "").await;
    assert_eq!(status(&output), 23, "{}", output.stderr_text());
    assert!(
        output.stderr_text().starts_with("tool error: file:"),
        "{}",
        output.stderr_text()
    );
    let kept = invoke(&ctx, DENIED_FILES, "", "cat /tmp/guarded/file").await;
    assert_eq!(kept.stdout, "owner-data", "{kept:?}");

    // Interactively, the denial is reported and the session goes on to the next line.
    let output = ssh(&ctx, &[DENIED_FILES], &format!("{script}\necho next\n")).await;
    assert_eq!(status(&output), 0, "{}", output.stderr_text());
    assert_eq!(output.stdout_text(), "next\n");
    assert!(
        output.stderr_text().contains("tool error: file:"),
        "{}",
        output.stderr_text()
    );
}

#[test]
#[timeout("20 minutes")]
async fn ssh_presents_a_siblings_stderr() {
    let ctx = context().await;
    let setup = "mkdir -p /tmp/ssh-stderr && cd /tmp/ssh-stderr && ";
    let both = "out-1\nout-2\nerr-1\nerr-2\n";

    // Each channel reaches its own process stream; redirections apply inside the script.
    for (script, stdout, stderr) in [
        ("fixture interleave ok", "out-1\nout-2\n", "err-1\nerr-2\n"),
        ("fixture interleave ok 2>/dev/null", "out-1\nout-2\n", ""),
        // Nothing records how the channels interleaved: stdout comes first.
        ("fixture interleave ok 2>&1", both, ""),
        ("fixture interleave ok |& cat", both, ""),
        (
            "fixture interleave ok | tr a-z A-Z",
            "OUT-1\nOUT-2\n",
            "err-1\nerr-2\n",
        ),
        ("fixture interleave ok 2>err; cat err", both, ""),
    ] {
        let output = ssh(&ctx, &[OWNER, "-c", &format!("{setup}{script}")], "").await;
        assert_output(&output, 0, stdout, stderr);
    }

    // A declared error follows the provider's stderr and sets the status.
    let output = ssh(&ctx, &[OWNER, "-c", "fixture interleave fail"], "").await;
    assert_output(
        &output,
        42,
        "out-1\nout-2\n",
        "err-1\nerr-2\ntool error: selected: \"interleave failure\"\n",
    );

    // Interactively, the failure is shown and the session goes on to the next command.
    let output = ssh(&ctx, &[OWNER], "fixture interleave fail\necho next\n").await;
    assert_output(
        &output,
        0,
        "out-1\nout-2\nnext\n",
        "err-1\nerr-2\ntool error: selected: \"interleave failure\"\n",
    );
}

// The prompt edits like a shell: unfinished input continues on a new line, Tab completes from
// the agent, and a later session recalls what an earlier one ran.
#[test]
#[timeout("20 minutes")]
async fn ssh_prompt_feels_like_a_shell() {
    let ctx = context().await;

    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        session.expect_str("Connected to")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        // An open here-document continues on a new line and runs as one command.
        enter(
            session,
            "mkdir -p /tmp/ssh-edit && cd /tmp/ssh-edit && cat > alpha.txt <<EOF",
        )?;
        session.expect_str("\u{b7} ")?;
        enter(session, "first line")?;
        enter(session, "EOF")?;
        answer_cursor_query(session)?;
        session.expect_str("/tmp/ssh-edit")?;

        // Tab completes a path on the agent, from the directory the session is in.
        session.send("cat al\t")?;
        session.expect_str("alpha.txt")?;
        enter(session, "")?;
        session.expect_str("first line")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        // Tab completes command names: the shell's own, and the tools bound to the agent.
        session.send("hostn\t")?;
        session.expect_str("hostname")?;
        session.send("\u{3}")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        session.send("fixtu\t")?;
        session.expect_str("fixture")?;
        session.send("\u{3}")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        enter(session, "echo recall-$((4800+21))")?;
        session.expect_str("recall-4821")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // A later session on the same agent recalls the last command with Up.
    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        session.send("\x1b[A")?;
        session.expect_str("4800+21")?;
        enter(session, "")?;
        session.expect_str("recall-4821")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // At a prompt, a directory removed from under the session sends it back to where it
    // started, and says so before the next command is typed.
    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "mkdir -p /tmp/ssh-gone && cd /tmp/ssh-gone")?;
        answer_cursor_query(session)?;
        session.expect_str("/tmp/ssh-gone")?;
        enter(session, "rmdir /tmp/ssh-gone")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "printf 'ran-%s\\n' here")?;
        session.expect_str("`invalid-cwd`")?;
        session.expect_str(
            "Continuing from the agent's starting directory; the command was not run.",
        )?;
        answer_cursor_query(session)?;
        session.expect_str("[255]")?;
        enter(session, "printf 'ran-%s\\n' \"$PWD\"")?;
        session.expect_str("ran-/\r\n")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // Two bash calls that sleep on one agent can fail that agent, an executor defect this test
    // must not depend on. So each agent below runs one sleeping command at most, and a quick
    // command first settles the session's own fetch of the command names.

    // A slow command shows the indicator, which is erased before the output is written.
    ctx.cli_interactive(["ssh", DENIED_FILES], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "true")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        enter(session, "sleep 2; printf 'sle%s\\n' pt")?;
        session.expect_str("Ctrl+C to stop waiting")?;
        session.expect_str("\x1b[2K")?;
        session.expect_str("slept")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // Ctrl+C stops waiting for a running command without ending the session.
    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "true")?;
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;

        enter(session, "sleep 6")?;
        session.expect_str("Ctrl+C to stop waiting")?;
        session.send("\u{3}")?;
        // Golem does not stop a call that has started, and says so without an error.
        session.expect_str("\x1b[2KStopped waiting. The command is still running on")?;
        session.expect_str("--lookup")?;
        answer_cursor_query(session)?;
        session.expect_str("[130]")?;

        // The session goes on. The next command is sent once the one left running has ended.
        std::thread::sleep(Duration::from_secs(10));
        enter(session, "printf 'aft%s\\n' er")?;
        session.expect_str("after")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // `help` and `tools` are answered by the session itself.
    ctx.cli_interactive(["ssh", OWNER], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "help")?;
        session.expect_str("How this session works")?;
        session.expect_str("exit [N]")?;
        answer_cursor_query(session)?;
        enter(session, "tools")?;
        session.expect_regex("  bash\r\n  fixture")?;
        session.expect_str("Run `NAME --help`")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;

    // An agent with only bash bound lists only bash.
    ctx.cli_interactive(["ssh", BASH_ONLY], move |session| {
        session.set_expect_timeout(Some(Duration::from_secs(120)));
        answer_cursor_query(session)?;
        session.expect_str(MARKER)?;
        enter(session, "tools")?;
        session.expect_regex("  bash\r\nRun `NAME --help`")?;
        answer_cursor_query(session)?;
        enter(session, "exit")?;
        session.expect_eof()
    })
    .await;
}
