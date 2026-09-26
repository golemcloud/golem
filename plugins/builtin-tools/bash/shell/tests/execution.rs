#![cfg(not(target_arch = "wasm32"))]

use bash_shell::{
    commands::{CommandDescriptor, CommandFuture, CommandInvoker, CommandOutput, PreparedCommand},
    session::Session,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

// The embedded uutils temporarily bind the process descriptors and working directory.
static SHELL: Mutex<()> = Mutex::new(());
fn run(future: impl std::future::Future<Output = ()>) {
    let _guard = SHELL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future);
}

#[test]
fn a_session_starts_in_a_checked_directory_as_if_launched_there() {
    run(async {
        let directory = tempfile::tempdir().unwrap();
        let dir = directory.path().display().to_string();
        let file = directory.path().join("file");
        std::fs::write(&file, "").unwrap();
        let file = file.display().to_string();
        let missing = directory.path().join("missing").display().to_string();
        let mut shell = Session::new().await.unwrap();
        for (bad, reason) in [
            ("relative", "cwd relative: not an absolute path".to_owned()),
            ("/tmp\0x", "cwd must not contain NUL bytes".to_owned()),
            (
                &missing,
                format!("cwd {missing}: No such file or directory"),
            ),
            (&file, format!("cwd {file}: Not a directory")),
        ] {
            assert_eq!(shell.set_working_dir(bad), Err(reason));
        }
        // No `cd` happened, so OLDPWD is whatever a new shell has, not the old directory.
        let fresh = shell.run("printf '%s|' \"${OLDPWD-unset}\"").await.stdout;
        let mut started = Session::new().await.unwrap();
        started.set_working_dir(&dir).unwrap();
        let result = started
            .run("printf '%s|' \"${OLDPWD-unset}\"; printf '%s|%s' \"$PWD\" \"$(pwd)\"")
            .await;
        let expected_tail = format!("{dir}|{dir}");
        assert_eq!(&result.stdout[..fresh.len()], fresh.as_slice());
        assert_eq!(&result.stdout[fresh.len()..], expected_tail.as_bytes());
        assert_eq!(result.cwd, dir);
    });
}

#[test]
fn local_mutations_and_pipelines_need_no_confirmation() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let mut shell = Session::new().await.unwrap();
        let result = shell.run(&format!("cd '{}'; mkdir nested; echo hello > nested/a; cp nested/a nested/b; mv nested/b nested/c; rm nested/a; cat nested/c | tr a-z A-Z", dir.path().display())).await;
        assert_eq!(
            result.exit_code,
            0,
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"HELLO\n");
        assert!(!dir.path().join("nested/a").exists());
        assert_eq!(result.cwd, dir.path().display().to_string());
        let help = shell.run("curl --help; wget --help").await;
        assert_eq!(help.exit_code, 0);
        assert!(help.stdout.starts_with(b"curl URL"));
        assert!(String::from_utf8_lossy(&help.stdout).contains("wget URL"));
    });
}

#[test]
fn finite_profile_rejects_coprocesses_and_refused_commands_wherever_they_appear() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        for script in [
            "coproc echo child",
            "eval 'coproc echo child'",
            "f() { coproc echo child; }; f",
            "echo before; cat <(umask)",
            "echo before; echo child > >(umask)",
            "echo before; cat <<EOF\n$(cat <(umask))\nEOF",
            "f() { :; } > >(umask); f",
        ] {
            let result = shell.run(script).await;
            assert_eq!(result.exit_code, 2, "{script}: {result:?}");
        }
        assert_eq!(
            shell.run("cat <<EOF\ncan't stop\nEOF").await.stdout,
            b"can't stop\n"
        );
    });
}

struct Invoker(Arc<AtomicUsize>);
struct Prepared {
    count: Arc<AtomicUsize>,
    input: bool,
    args: Vec<String>,
}
impl CommandInvoker for Invoker {
    fn prepare(&self, _: &str, argv: &[String]) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
        if argv.first().is_some_and(|a| a == "--help") {
            return Err(CommandOutput {
                stdout: b"probe help\n".to_vec(),
                ..Default::default()
            });
        }
        Ok(Box::new(Prepared {
            count: self.0.clone(),
            input: argv.first().is_some_and(|a| a == "--stdin"),
            args: argv.to_vec(),
        }))
    }
}
impl PreparedCommand for Prepared {
    fn takes_stdin(&self) -> bool {
        self.input
    }
    fn invoke(&self, stdin: Option<Vec<u8>>) -> CommandFuture<'_> {
        Box::pin(async move {
            self.count.fetch_add(1, Ordering::SeqCst);
            CommandOutput {
                stdout: stdin.unwrap_or_else(|| self.args.join("|").into_bytes()),
                ..Default::default()
            }
        })
    }
}

#[test]
fn bound_commands_use_expanded_arguments_and_only_declared_input() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let shadowed = shell
            .register_commands(
                vec![
                    CommandDescriptor {
                        name: "probe".into(),
                        help: "probe help".into(),
                    },
                    CommandDescriptor {
                        name: "cd".into(),
                        help: String::new(),
                    },
                ],
                Arc::new(Invoker(calls.clone())),
            )
            .unwrap();
        assert_eq!(shadowed, ["cd"]);
        assert_eq!(
            shell
                .run("name='two words'; probe \"$name\" third")
                .await
                .stdout,
            b"two words|third"
        );
        assert_eq!(
            shell.run("echo payload | probe --stdin").await.stdout,
            b"payload\n"
        );
        assert_eq!(
            shell.run("echo ignored | probe --help").await.stdout,
            b"probe help\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            shell.run("f() { probe nested; }; f").await.stdout,
            b"nested"
        );
    });
}

#[test]
fn oversized_bound_command_input_never_invokes_the_adapter() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        shell
            .register_commands(
                vec![CommandDescriptor {
                    name: "probe".into(),
                    help: String::new(),
                }],
                Arc::new(Invoker(calls.clone())),
            )
            .unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file()
            .set_len((bash_shell::commands::MAX_STDIN_BYTES + 1) as u64)
            .unwrap();
        let result = shell
            .run(&format!("probe --stdin < '{}'", file.path().display()))
            .await;
        assert_eq!(result.exit_code, 2);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn preflight_allows_jobs_but_refuses_coprocesses_and_unsupported_job_control() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        assert_eq!(shell.run("true & wait; echo ok").await.stdout, b"ok\n");
        let coproc = shell.run("coproc cat").await;
        assert_eq!(coproc.exit_code, 2);
        assert_eq!(
            coproc.stderr,
            b"bash: background coprocesses are unsupported in bash-tool\n"
        );
        let disown = shell.run("disown").await;
        assert_eq!(disown.exit_code, 1);
        assert_eq!(
            String::from_utf8_lossy(&disown.stderr),
            "bash: line 1: disown: current: no such job\n"
        );
        for script in [
            "fg",
            "bg",
            "umask",
            "wait -p v",
            "wait -f",
            "echo before; suspend -f",
        ] {
            let refused = shell.run(script).await;
            assert_eq!(refused.exit_code, 2, "{script}");
            assert!(
                String::from_utf8_lossy(&refused.stderr).contains("unsupported in bash-tool"),
                "{script}"
            );
            assert!(refused.stdout.is_empty(), "{script}");
        }
    });
}

#[test]
fn find_exec_runs_isolated_child_commands() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let mut shell = Session::new().await.unwrap();
        let result = shell
            .run(&format!(
                "cd '{}'; mkdir -p a/b; printf 'main\\n' > a/x.rs; : > a/b/y.rs; \
                 find . -name '*.rs' -exec echo hit {{}} \\; | sort; \
                 find . -name '*.rs' -exec echo {{}} + | tr ' ' '\\n' | sort; \
                 find . -type f -exec grep -q main {{}} \\; -print; \
                 find . -name y.rs -execdir pwd \\; | sed 's|.*/||'; \
                 find . -maxdepth 0 -exec cd / \\; ; pwd | sed 's|.*/||'; \
                 find . -maxdepth 0 -exec nosuchcmd {{}} \\; ; echo status=$?",
                dir.path().display()
            ))
            .await;
        let base = dir.path().file_name().unwrap().to_string_lossy();
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            format!(
                "hit ./a/b/y.rs\nhit ./a/x.rs\n./a/b/y.rs\n./a/x.rs\n./a/x.rs\nb\n{base}\nstatus=0\n"
            )
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stderr),
            "find: \u{2018}nosuchcmd\u{2019}: No such file or directory\n"
        );
    });
}

#[test]
#[cfg(unix)]
fn find_follows_symlinks_only_when_asked_and_detects_loops() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("L");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/f"), "x").unwrap();
        std::os::unix::fs::symlink("..", root.join("a/loop")).unwrap();
        std::os::unix::fs::symlink("../nonexist", root.join("broken")).unwrap();
        std::os::unix::fs::symlink("a", root.join("tolink")).unwrap();
        let mut shell = Session::new().await.unwrap();
        let cd = format!("cd '{}'", dir.path().display());
        let physical = shell.run(&format!("{cd}; find L -type l | sort")).await;
        assert_eq!(physical.stdout, b"L/a/loop\nL/broken\nL/tolink\n");
        let xtype = shell.run(&format!("{cd}; find L -xtype l")).await;
        assert_eq!(xtype.stdout, b"L/broken\n");
        let kinds = shell
            .run(&format!(
                "{cd}; find L -mindepth 1 -maxdepth 1 -printf '%p %y %Y %l\\n' | sort"
            ))
            .await;
        assert_eq!(
            String::from_utf8_lossy(&kinds.stdout),
            "L/a d d \nL/broken l N ../nonexist\nL/tolink l d a\n"
        );
        let logical = shell.run(&format!("{cd}; find -L L | sort")).await;
        let status = shell
            .run(&format!("{cd}; find -L L >/dev/null 2>&1; echo $?"))
            .await;
        assert_eq!(status.stdout, b"1\n");
        assert_eq!(
            String::from_utf8_lossy(&logical.stdout),
            "L\nL/a\nL/a/f\nL/broken\nL/tolink\nL/tolink/f\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&logical.stderr),
            "find: File system loop detected; \u{2018}L/a/loop\u{2019} is part of the same file \
             system loop as \u{2018}L\u{2019}.\nfind: File system loop detected; \
             \u{2018}L/tolink/loop\u{2019} is part of the same file system loop as \
             \u{2018}L\u{2019}.\n"
        );
        let command_line = shell.run(&format!("{cd}; find -H L/tolink | sort")).await;
        assert_eq!(
            command_line.stdout,
            b"L/tolink\nL/tolink/f\nL/tolink/loop\n"
        );
    });
}

#[test]
fn xargs_builds_command_lines_and_reports_gnu_statuses() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let result = shell
            .run(
                "printf 'a b\\nc\\n' | xargs -n 2 echo; \
                 printf 'x\\0y z\\0' | xargs -0 -I{} echo '[{}]'; \
                 printf '' | xargs echo empty; \
                 printf '' | xargs -r echo never; \
                 printf 'a\\n' | xargs false; echo status=$?; \
                 printf 'a\\n' | xargs nosuchcmd; echo status=$?; \
                 printf '/\\n' | xargs cd; pwd >/dev/null; echo done",
            )
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "a b\nc\n[x]\n[y z]\nempty\nstatus=123\nstatus=127\ndone\n"
        );
        assert_eq!(
            String::from_utf8_lossy(&result.stderr),
            "xargs: nosuchcmd: No such file or directory\n"
        );
    });
}

// -- curl/wget stream their response body through the shell's real stdout instead of
// buffering it first. These run the actual `execute_http` path end to end (through `Session`,
// native only — wasmtime 46 cannot execute HTTP at all right now, a pre-existing, unrelated
// limitation), against a hermetic localhost server.
//
// What these do NOT cover, and why: the end-to-end proof of "chunks are forwarded before EOF" is
// an endless body piped through `head -c N`. It cannot run here: natively, `head` and the other
// commands are uutils' finite versions, which read their whole input first (the record-at-a-time
// drivers are WASM-only), so even `while :; do echo x; done | head -n 1` never ends through a
// native `Session`. That proof runs against the real component instead, in
// `cli/golem-cli/tests/app/builtin_bash.rs` (`/trickle`).
// The exact property that matters here — a closed sink ends the transfer immediately, without
// reading to the server's declared EOF — IS proven, deterministically and without depending on
// brush's own pipe plumbing, at the `wcurl`/`waget` unit level: see
// `streaming_propagates_a_sink_write_error_instead_of_swallowing_it` in both crates, which uses a
// sink that fails on demand instead of relying on a real OS pipe. What's left to prove here is
// end-to-end CORRECTNESS through the real `execute_http` path: `head -c N` truncation, `-o`/`-O`
// incremental file writes, `wget -O -`, and `wget -c`.

use std::io::{Read, Write};
use std::net::TcpListener;

/// A one-shot server that serves a whole (finite) body, `Connection: close`d after — a body large
/// enough that a naive implementation reading everything before forwarding anything would still
/// be observably slower, without the hang risk of a server that never finishes.
fn body_server(body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

async fn with_timeout(seconds: u64, future: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(std::time::Duration::from_secs(seconds), future)
        .await
        .expect("timed out");
}

#[test]
fn curl_truncated_by_head_completes_through_the_real_shell_pipeline() {
    run(async {
        with_timeout(10, async {
            let mut shell = Session::new().await.unwrap();
            let body: Vec<u8> = (0..500_000).map(|i| b'a' + (i % 26) as u8).collect();
            let body: &'static [u8] = Box::leak(body.into_boxed_slice());
            let url = body_server(body);
            let result = shell
                .run(&format!("curl -s {url} | head -c 5 | wc -c"))
                .await;
            assert_eq!(
                result.exit_code,
                0,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "5");
        })
        .await;
    });
}

#[test]
fn wget_dash_o_dash_streams_to_stdout_through_the_real_shell_pipeline() {
    run(async {
        with_timeout(10, async {
            let mut shell = Session::new().await.unwrap();
            let url = body_server(b"streamed-to-stdout");
            let result = shell.run(&format!("wget -q -O - {url}")).await;
            assert_eq!(
                result.exit_code,
                0,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(result.stdout, b"streamed-to-stdout");
        })
        .await;
    });
}

#[test]
fn curl_output_flag_writes_a_large_body_incrementally_and_correctly() {
    run(async {
        with_timeout(10, async {
            let dir = tempfile::tempdir().unwrap();
            let mut shell = Session::new().await.unwrap();
            let body: Vec<u8> = (0..2_000_000).map(|i| (i % 251) as u8).collect();
            let body: &'static [u8] = Box::leak(body.into_boxed_slice());
            let url = body_server(body);
            let result = shell
                .run(&format!(
                    "cd '{}'; curl -s -o out.bin {url}; wc -c < out.bin",
                    dir.path().display()
                ))
                .await;
            assert_eq!(
                result.exit_code,
                0,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "2000000");
            assert_eq!(std::fs::read(dir.path().join("out.bin")).unwrap(), body);
        })
        .await;
    });
}

#[test]
fn wget_continue_appends_through_the_full_shell_pipeline() {
    run(async {
        with_timeout(10, async {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("partial.bin"), b"first-half-").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 2048];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                    assert!(
                        request.contains("range: bytes=11-"),
                        "expected a Range request:\n{request}"
                    );
                    let body = b"second-half";
                    let head = format!(
                        "HTTP/1.1 206 X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                    let _ = stream.flush();
                }
            });
            let mut shell = Session::new().await.unwrap();
            let result = shell
                .run(&format!(
                    "cd '{}'; wget -q -c -O partial.bin http://{addr}/",
                    dir.path().display()
                ))
                .await;
            assert_eq!(
                result.exit_code,
                0,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(dir.path().join("partial.bin")).unwrap(),
                "first-half-second-half"
            );
        })
        .await;
    });
}

#[test]
fn exec_redirections_reach_later_commands() {
    run(async {
        let mut shell = Session::new().await.unwrap();

        // `exec 2>&1` duplicates stderr onto stdout for the rest of the script, for Brush's own
        // builtins and for the embedded commands alike.
        let merged = shell
            .run("exec 2>&1; echo builtin >&2; ls /definitely-missing; echo after")
            .await;
        let stdout = String::from_utf8_lossy(&merged.stdout);
        assert!(stdout.starts_with("builtin\n"), "{stdout:?}");
        assert!(stdout.contains("ls: cannot access"), "{stdout:?}");
        assert!(stdout.ends_with("after\n"), "{stdout:?}");
        assert_eq!(
            merged.stderr,
            b"",
            "{:?}",
            String::from_utf8_lossy(&merged.stderr)
        );

        // The other direction, in a fresh run of the same session.
        let swapped = shell.run("exec 1>&2; echo moved").await;
        assert_eq!(swapped.stdout, b"");
        assert_eq!(swapped.stderr, b"moved\n");

        // A later run starts from the invocation's own streams again.
        let fresh = shell.run("echo out; echo err >&2").await;
        assert_eq!(fresh.stdout, b"out\n");
        assert_eq!(fresh.stderr, b"err\n");
    });
}

#[test]
fn sh_c_runs_scripts_as_isolated_children() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let result = shell
            .run(
                "cd /tmp; sh -c 'echo \"$1-$2\" $#; cd /; x=1; exit 3' _ a 'b c'; \
                 echo status=$? pwd=$PWD x=${x-unset}",
            )
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "a-b c 2\nstatus=3 pwd=/tmp x=unset\n"
        );

        // stdin reaches the script, set options apply, and a script can come from stdin.
        let result = shell
            .run(
                "echo piped | bash -c 'cat'; bash -ec 'false; echo not-reached'; echo e=$?; \
                 echo 'echo from-stdin \"$1\"' | sh -s arg",
            )
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "piped\ne=1\nfrom-stdin arg\n"
        );

        // The script passes the same checks as a top-level script, before any of it runs.
        let refused = shell.run("sh -c 'echo ran; umask'; echo status=$?").await;
        assert_eq!(refused.stdout, b"status=2\n");
        assert_eq!(
            String::from_utf8_lossy(&refused.stderr),
            "sh: umask is unsupported in bash-tool\n"
        );

        // The forms that motivate it.
        let directory = tempfile::tempdir().unwrap();
        let result = shell
            .run(&format!(
                "cd '{}'; touch 'a b' c; find . -type f -exec sh -c 'echo \"<$1>\"' _ {{}} \\; | sort; \
                 printf '1\\n2\\n' | xargs -n 1 bash -c 'echo $(( $1 * 10 ))' _",
                directory.path().display()
            ))
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "<./a b>\n<./c>\n10\n20\n"
        );
    });
}

#[test]
fn bash_run_reaches_the_bound_bash_tool_without_shadowing_it() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let shadowed = shell
            .register_commands(
                vec![CommandDescriptor {
                    name: "bash".into(),
                    help: String::new(),
                }],
                Arc::new(Invoker(calls.clone())),
            )
            .unwrap();
        assert!(shadowed.is_empty(), "{shadowed:?}");
        let result = shell
            .run("bash run --cwd / 'printf nested'; bash -c 'echo local'")
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "run|--cwd|/|printf nestedlocal\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn exit_traps_run_when_the_script_ends_and_children_start_without_traps() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let ended = shell.run("trap 'echo bye' EXIT; echo hi").await;
        assert_eq!(ended.stdout, b"hi\nbye\n");

        let exited = shell.run("trap 'echo bye $?' EXIT; exit 3").await;
        assert_eq!(exited.stdout, b"bye 3\n");
        assert_eq!(exited.exit_code, 3);

        // A child is a new process: it does not run the caller's EXIT or ERR traps, but runs its
        // own EXIT trap when it ends.
        let children = shell
            .run(
                "trap 'echo parent-exit' EXIT; trap 'echo parent-err' ERR; \
                 find /tmp -maxdepth 0 -exec false \\; ; \
                 sh -c 'trap \"echo child-exit\" EXIT; echo in-child'; echo done",
            )
            .await;
        assert_eq!(
            String::from_utf8_lossy(&children.stdout),
            "in-child\nchild-exit\ndone\nparent-exit\n"
        );
    });
}

#[test]
fn uutils_run_in_process_with_their_own_names_statuses_and_cwd() {
    run(async {
        let directory = tempfile::tempdir().unwrap();
        let mut shell = Session::new().await.unwrap();

        // Named after the utility, translated, with the usage hint: not the host program's name
        // or a raw message key.
        let named = shell.run("dirname; ls /definitely-missing").await;
        assert_eq!(
            String::from_utf8_lossy(&named.stderr),
            "dirname: missing operand\nTry 'dirname --help' for more information.\n\
             ls: cannot access '/definitely-missing': No such file or directory\n"
        );

        // A failure does not leak into the next command's status, relative operands resolve
        // against the shell's cwd, and a usage error returns instead of ending the process.
        let result = shell
            .run(&format!(
                "ls /definitely-missing 2>/dev/null; cd '{}'; touch made; echo status=$?; ls; \
                 mv made 2>/dev/null; echo mv=$?; mktemp --bogus 2>/dev/null; echo mktemp=$?",
                directory.path().display()
            ))
            .await;
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "status=0\nmade\nmv=1\nmktemp=1\n"
        );
    });
}

/// A bound tool whose behaviour follows its first argument: `fail` exits 7 with a message,
/// `usage` is rejected before any call, `slow` yields before answering; anything else echoes.
struct Scripted(Arc<AtomicUsize>);
struct ScriptedCall {
    calls: Arc<AtomicUsize>,
    args: Vec<String>,
}
impl CommandInvoker for Scripted {
    fn prepare(&self, _: &str, argv: &[String]) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
        if argv.first().is_some_and(|a| a == "usage") {
            return Err(CommandOutput {
                stderr: b"scripted: unknown subcommand\n".to_vec(),
                exit_code: 2,
                ..Default::default()
            });
        }
        Ok(Box::new(ScriptedCall {
            calls: self.0.clone(),
            args: argv.to_vec(),
        }))
    }
}
impl PreparedCommand for ScriptedCall {
    fn takes_stdin(&self) -> bool {
        false
    }
    fn invoke(&self, _: Option<Vec<u8>>) -> CommandFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.args.first().map(String::as_str) {
                Some("fail") => CommandOutput {
                    stderr: b"tool error: boom\n".to_vec(),
                    exit_code: 7,
                    ..Default::default()
                },
                Some("slow") => {
                    for _ in 0..10 {
                        tokio::task::yield_now().await;
                    }
                    CommandOutput {
                        stdout: format!("{}\n", self.args.join(" ")).into_bytes(),
                        ..Default::default()
                    }
                }
                _ => CommandOutput {
                    stdout: format!("{}\n", self.args.join(" ")).into_bytes(),
                    ..Default::default()
                },
            }
        })
    }
}

#[test]
fn bound_command_failures_reach_the_script() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        shell
            .register_commands(
                vec![CommandDescriptor {
                    name: "scripted".into(),
                    help: String::new(),
                }],
                Arc::new(Scripted(calls.clone())),
            )
            .unwrap();

        // A failed call sets the status and writes stderr; the script carries on.
        let failed = shell
            .run("scripted fail; echo status=$?; scripted fail 2>/dev/null || echo handled")
            .await;
        assert_eq!(failed.stdout, b"status=7\nhandled\n");
        assert_eq!(failed.stderr, b"tool error: boom\n");

        // Rejected arguments never reach the tool.
        let before = calls.load(Ordering::SeqCst);
        let rejected = shell.run("scripted usage; echo status=$?").await;
        assert_eq!(rejected.stdout, b"status=2\n");
        assert_eq!(rejected.stderr, b"scripted: unknown subcommand\n");
        assert_eq!(calls.load(Ordering::SeqCst), before);

        // In a pipeline the failure is the stage's status, and pipefail sees it.
        let piped = shell
            .run("set -o pipefail; scripted fail 2>/dev/null | cat; echo status=$? stages=${PIPESTATUS[*]}")
            .await;
        assert_eq!(piped.stdout, b"status=7 stages=7 0\n");
    });
}

#[test]
fn bound_commands_run_concurrently_in_background_jobs() {
    run(async {
        let mut shell = Session::new().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        shell
            .register_commands(
                vec![CommandDescriptor {
                    name: "scripted".into(),
                    help: String::new(),
                }],
                Arc::new(Scripted(calls.clone())),
            )
            .unwrap();
        let result = shell
            .run(
                "scripted slow one > /tmp/bound-one & scripted slow two > /tmp/bound-two & \
                 scripted fail 2>/dev/null & wait; cat /tmp/bound-one /tmp/bound-two",
            )
            .await;
        // Job statuses come from the WASM process model, so the matrix checks them there.
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "slow one\nslow two\n"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    });
}
