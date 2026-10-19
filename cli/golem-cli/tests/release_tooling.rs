// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use test_r::test;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn release_source_requires_the_exact_merged_pr_head() {
    let fixture = TempDir::new().unwrap();
    let repo = fixture.path().join("source");
    let remote = fixture.path().join("public.git");
    let bin = fixture.path().join("bin");
    let response = fixture.path().join("pulls.json");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::copy(
        crate::workspace_path().join("Makefile.toml"),
        repo.join("Makefile.toml"),
    )
    .unwrap();
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "release-test"]);
    git(
        &repo,
        &["config", "user.email", "release-test@example.invalid"],
    );
    git(&repo, &["config", "commit.gpgsign", "false"]);
    git(&repo, &["config", "tag.gpgsign", "false"]);
    git(&repo, &["config", "core.hooksPath", "/dev/null"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "baseline"]);
    git(&repo, &["checkout", "-b", "release"]);
    fs::write(repo.join("release.txt"), "release source").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "prepare release"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["tag", "v9.8.7-rc1"]);
    git(&repo, &["checkout", "main"]);
    fs::write(repo.join("development.txt"), "not in the release").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "concurrent development"]);
    git(&repo, &["merge", "--squash", "release"]);
    git(&repo, &["commit", "-m", "squash release PR"]);
    let merged = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "later development"],
    );
    git(&repo, &["clone", "--bare", ".", remote.to_str().unwrap()]);
    git(&repo, &["checkout", "release"]);
    git(
        &repo,
        &[
            "config",
            &format!("url.{}.insteadOf", remote.display()),
            "https://github.com/golemcloud/golem.git",
        ],
    );
    assert!(!repo.join("development.txt").exists());
    assert!(
        !Command::new("git")
            .current_dir(&repo)
            .args(["merge-base", "--is-ancestor", &head, "main"])
            .status()
            .unwrap()
            .success()
    );

    // Supply API responses while running the production jq selector and real Git checks.
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
[ "$GH_TOKEN" = "release-read-token" ] || exit 1
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--jq" ]; then
        exec jq -r "$2" "$RELEASE_TEST_PULLS"
    fi
    shift
done
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let validate = |pulls: serde_json::Value, expected: bool| {
        fs::write(&response, pulls.to_string()).unwrap();
        let output = Command::new("cargo")
            .current_dir(&repo)
            .args([
                "make",
                "--disable-check-for-updates",
                "--no-workspace",
                "--profile",
                "development",
                "validate-golem-release-source",
            ])
            .env_remove("CI")
            .env("GITHUB_TOKEN", "release-read-token")
            .env("GH_TOKEN", "release-write-token")
            .env("BUILTIN_VERSION", "9.8.7-rc1")
            .env("RELEASE_TEST_PULLS", &response)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{pulls}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let pull = json!({
        "merged_at": "2026-10-18T12:00:00Z",
        "head": { "sha": head },
        "base": { "ref": "main", "repo": { "full_name": "golemcloud/golem" } },
        "merge_commit_sha": merged,
    });
    validate(json!([pull]), true);
    validate(json!([]), false);
    validate(json!(null), false);
    for (pointer, value) in [
        ("/merged_at", json!(null)),
        ("/head/sha", json!(merged)),
        ("/base/ref", json!("1.5.x")),
        ("/base/repo/full_name", json!("fork/golem")),
        ("/merge_commit_sha", json!(head)),
    ] {
        let mut rejected = pull.clone();
        *rejected.pointer_mut(pointer).unwrap() = value;
        validate(json!([rejected]), false);
        validate(json!([rejected, pull]), true);
    }
    fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
    validate(json!([pull]), false);
    fs::remove_file(repo.join("dirty.txt")).unwrap();
    git(&repo, &["tag", "--force", "v9.8.7-rc1", &merged]);
    validate(json!([pull]), false);
    git(&repo, &["checkout", "--detach", &merged]);
    validate(json!([pull]), false);
}

#[test]
fn release_packages_and_readiness_preserve_immutable_outputs() {
    let output = Command::new("python3")
        .arg(crate::workspace_path().join("cli/golem-cli/tests/release_flow.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
