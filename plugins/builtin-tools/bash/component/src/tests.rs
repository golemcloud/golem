use super::*;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn bad_cwds_are_rejected_before_the_script_runs() {
    let base = std::env::temp_dir().join(format!("bash-tool-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("file");
    std::fs::write(&file, "").unwrap();
    let marker = base.join("ran");
    let missing = base.join("missing");
    let script = format!("touch '{}'", marker.display());
    for (cwd, reason) in [
        (
            "relative/dir".to_owned(),
            "cwd relative/dir: not an absolute path".to_owned(),
        ),
        (
            "/tmp\0x".to_owned(),
            "cwd must not contain NUL bytes".to_owned(),
        ),
        (
            missing.display().to_string(),
            format!("cwd {}: No such file or directory", missing.display()),
        ),
        (
            file.display().to_string(),
            format!("cwd {}: Not a directory", file.display()),
        ),
    ] {
        match block_on(run_script(&cwd, &script, DEFAULT_TIMEOUT)) {
            Err(BashError::InvalidCwd { reason: got }) => assert_eq!(got, reason),
            other => panic!("{cwd:?}: {other:?}"),
        }
        assert!(!marker.exists(), "{cwd:?} ran the script");
    }
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn a_time_limit_outside_one_second_to_an_hour_is_rejected_before_the_script_runs() {
    let marker = std::env::temp_dir().join(format!("bash-tool-timeout-{}", std::process::id()));
    let script = format!("touch '{}'", marker.display());
    for timeout in [0, MAX_TIMEOUT + 1] {
        match block_on(run_script("", &script, timeout)) {
            Err(BashError::InvalidTimeout { reason }) => assert_eq!(
                reason,
                format!("timeout {timeout}: must be between 1 and 3600 seconds")
            ),
            other => panic!("{timeout}: {other:?}"),
        }
        assert!(!marker.exists(), "{timeout} ran the script");
    }
    assert_eq!(
        block_on(run_script("", "exit 3", MAX_TIMEOUT))
            .unwrap()
            .exit_code,
        3
    );
}

#[test]
fn every_call_is_a_fresh_shell_that_starts_in_the_given_cwd() {
    block_on(async {
        let first = run_script(
            "",
            "cd /tmp; x=kept; f() { echo function; }; alias ll='echo alias'; set -o pipefail; \
             trap 'echo caught' ERR; set -- a b; pushd / >/dev/null; (exit 7)",
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(first.exit_code, 7, "{}", first.stderr);
        assert_eq!(first.cwd, "/");
        assert!(first.stderr.is_empty(), "{}", first.stderr);

        let second = run_script(
            "/tmp",
            "printf '[%s]' \"$?\" \"${x-unset}\" \"$#\" \"$PWD\" \"$(dirs)\"; \
             type f ll 2>/dev/null || printf '[none]'; [[ -o pipefail ]] || printf '[no-pipefail]'; \
             false",
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(
            second.stdout, "[0][unset][0][/tmp][/tmp][none][no-pipefail]",
            "{}",
            second.stderr
        );
        assert_eq!(second.cwd, "/tmp");
        // `false` fails without printing `caught`: the first call's ERR trap is gone.
        assert_eq!(second.exit_code, 1);
        // The ERR trap runs within its own call. (`$$` is only seeded on WASM; natively Brush
        // reports the host's process id, so its per-call freshness is tested in Golem.)
        assert_eq!(first.stdout, "caught\n");

        for script in [
            "coproc echo hidden",
            "eval 'coproc echo hidden'",
            "f() { coproc echo hidden; }; f",
            "echo \"$(umask)\"",
            "trap 'coproc echo hidden' PIPE",
        ] {
            let refused = run_script("", script, DEFAULT_TIMEOUT).await.unwrap();
            assert_eq!(refused.exit_code, 2, "{script}: {}", refused.stderr);
            assert!(refused.stdout.is_empty(), "{script}: {}", refused.stdout);
        }
    });
}

#[test]
fn new_shell_pids_cover_the_documented_range() {
    assert_eq!(new_shell_pid(0), 1_000);
    assert_eq!(new_shell_pid(4_194_303 - 1_000), 4_194_303);
    assert_eq!(new_shell_pid(4_194_303 - 1_000 + 1), 1_000);
}
