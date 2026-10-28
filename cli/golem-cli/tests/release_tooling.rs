// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use test_r::test;

#[test]
fn draft_notes_start_at_the_previous_stable_golem_release() {
    let fixture = TempDir::new().unwrap();
    let workflow: serde_yaml::Value = serde_yaml::from_str(
        &fs::read_to_string(
            crate::workspace_path().join(".github/workflows/prepare-golem-release.yaml"),
        )
        .unwrap(),
    )
    .unwrap();
    let steps = workflow["jobs"]["publish-builtins"]["steps"]
        .as_sequence()
        .unwrap();
    let script = steps
        .iter()
        .find(|step| step["name"].as_str() == Some("Create or resume the draft"))
        .unwrap()["run"]
        .as_str()
        .unwrap();
    let gh = fixture.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
set -eu
case "$1 $2" in
    'release view') exit "$DRAFT_EXISTS" ;;
    'api --paginate') cat "$RELEASE_TEST_PAGES" ;;
    'release create') printf '%s\n' "$@" ;;
    *) exit 2 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let pages = fixture.path().join("pages.json");
    // Separate pages deliberately mix publication order, release families, and version order.
    fs::write(
        &pages,
        r#"[
          {"tag_name":"v1.6.2","draft":true,"prerelease":false},
          {"tag_name":"golem-shared-v1.6.1","draft":false,"prerelease":false},
          {"tag_name":"v1.6.1-rc1","draft":false,"prerelease":true},
          {"tag_name":"v1.5.9","draft":false,"prerelease":false},
          {"tag_name":"v1.5.10","draft":false,"prerelease":false}
        ]
        [
          {"tag_name":"golem-rust-v3.0.1","draft":false,"prerelease":false},
          {"tag_name":"v2.0.0","draft":false,"prerelease":false},
          {"tag_name":"v1.6.0","draft":false,"prerelease":false},
          {"tag_name":"v1.6.1","draft":false,"prerelease":false}
        ]"#,
    )
    .unwrap();
    let path = std::env::join_paths(
        std::iter::once(fixture.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for (tag, previous, exists) in [
        ("v1.6.1", Some("v1.6.0"), false),
        ("v1.6.1-rc2", Some("v1.6.0"), false),
        ("v1.5.11", Some("v1.5.10"), false),
        ("v1.0.0", None, false),
        ("v1.6.1", None, true),
    ] {
        let output = Command::new("bash")
            .args(["-c", script])
            .env("PATH", &path)
            .env("GITHUB_REF_NAME", tag)
            .env("RELEASE_TEST_PAGES", &pages)
            .env("DRAFT_EXISTS", if exists { "0" } else { "1" })
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let args: Vec<_> = stdout.lines().collect();
        if exists {
            assert!(args.is_empty(), "an existing draft must not be changed");
        } else {
            assert!(args.contains(&"--generate-notes"));
            assert!(args.contains(&"--latest=false"));
            let actual = args
                .windows(2)
                .find(|pair| pair[0] == "--notes-start-tag")
                .map(|pair| pair[1]);
            assert_eq!(actual, previous, "{tag}: {stdout}");
        }
    }
}

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
fn release_channels_use_published_versions_in_each_workflow() {
    let fixture = TempDir::new().unwrap();
    for (name, script) in [
        (
            "gh",
            r#"#!/bin/sh
set -eu
case "$1 $2" in
  'api --paginate')
    [ "$RELEASE_TEST_LOOKUP_FAILS" != all ] || exit 1
    while [ "$1" != --jq ]; do shift; done
    exec jq -r "$2" "$RELEASE_TEST_PAGES" ;;
  'release edit') printf '%s\n' "$@" ;;
  *) exit 1 ;;
esac
"#,
        ),
        (
            "npm",
            r#"#!/bin/sh
set -eu
case "$1" in
  view)
    [ "$RELEASE_TEST_LOOKUP_FAILS" != all ] && [ "$RELEASE_TEST_LOOKUP_FAILS" != "$2" ] || exit 1
    case "$2" in
      @golemcloud/golem-ts-sdk|@golemcloud/golem-ts-bridge|@golemcloud/golem-ts-repl|@golemcloud/effect-golem) ;;
      *) exit 1 ;;
    esac
    if [ "$2" = @golemcloud/golem-ts-bridge ]; then
      jq 'map(select(. != "2.0.0"))' "$RELEASE_TEST_VERSIONS"
    else
      cat "$RELEASE_TEST_VERSIONS"
    fi ;;
  publish) printf '%s\n' "$@" ;;
  *) exit 1 ;;
esac
"#,
        ),
        ("pnpm", "#!/bin/sh\nprintf '%s\\n' \"$@\"\n"),
    ] {
        let file = fixture.path().join(name);
        fs::write(&file, script).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(fixture.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    for (version, newer_stable, expected, lookup_failure, npm_downgrade) in [
        ("1.6.11", false, "latest", "none", false),
        ("1.6.11", true, "release-1.6", "none", false),
        ("2.0.0-rc2", false, "next", "none", false),
        ("1.6.11", false, "", "all", false),
        (
            "1.6.11",
            false,
            "latest",
            "@golemcloud/golem-ts-bridge",
            false,
        ),
        (
            "1.6.11",
            false,
            "latest",
            "@golemcloud/golem-ts-repl",
            false,
        ),
        ("1.6.9", true, "release-1.6", "none", true),
        ("2.0.0-rc0", false, "next", "none", true),
    ] {
        let pages = fixture.path().join("pages.json");
        fs::write(
            &pages,
            json!([
                {"tag_name":"v1.6.10", "draft":false, "prerelease":false},
                {"tag_name":"v2.0.0-rc1", "draft":false, "prerelease":true},
                {"tag_name":"v3.0.0", "draft":true, "prerelease":false},
                {"tag_name":"golem-shared-v8.0.0", "draft":false, "prerelease":false},
                {"tag_name":"v2.0.0", "draft":!newer_stable, "prerelease":false}
            ])
            .to_string(),
        )
        .unwrap();
        let versions = fixture.path().join("versions.json");
        fs::write(
            &versions,
            if newer_stable {
                r#"["1.6.10","2.0.0-rc1","2.0.0"]"#
            } else {
                r#"["1.6.10","2.0.0-rc1"]"#
            },
        )
        .unwrap();
        for (file, job, step_name, directory, tag) in [
            (
                ".github/actions/docker-publish-setup/action.yml",
                "",
                "Determine whether to publish ':latest'",
                "",
                format!("v{version}"),
            ),
            (
                ".github/workflows/publish-golem-release.yaml",
                "preparation",
                "Set the stable release channel",
                "",
                format!("v{version}"),
            ),
            (
                ".github/workflows/publish-golem-ts.yaml",
                "publish",
                "Publish all packages",
                "sdks/ts",
                format!("golem-ts-v{version}"),
            ),
            (
                ".github/workflows/publish-golem-effect.yaml",
                "publish",
                "Publish with provenance",
                "sdks/effect",
                format!("golem-effect-v{version}"),
            ),
        ] {
            let yaml: serde_yaml::Value = serde_yaml::from_str(
                &fs::read_to_string(crate::workspace_path().join(file)).unwrap(),
            )
            .unwrap();
            let steps = if job.is_empty() {
                &yaml["runs"]["steps"]
            } else {
                &yaml["jobs"][job]["steps"]
            };
            let script = steps
                .as_sequence()
                .unwrap()
                .iter()
                .find(|step| step["name"].as_str() == Some(step_name))
                .unwrap()["run"]
                .as_str()
                .unwrap()
                .replace("${{ steps.get_version.outputs.version }}", version);
            let env_file = fixture.path().join("env");
            fs::write(&env_file, "").unwrap();
            let output = Command::new("bash")
                .args(["-e", "-o", "pipefail", "-c", &script])
                .current_dir(crate::workspace_path().join(directory))
                .env("PATH", &path)
                .env("RELEASE_TEST_PAGES", &pages)
                .env("RELEASE_TEST_VERSIONS", &versions)
                .env("RELEASE_TEST_LOOKUP_FAILS", lookup_failure)
                .env("RELEASE_TAG", &tag)
                .env("GITHUB_EVENT_NAME", "release")
                .env("GITHUB_REF_NAME", &tag)
                .env("GITHUB_ENV", &env_file)
                .output()
                .unwrap();
            let should_fail = lookup_failure == "all"
                || (directory == "sdks/ts" && lookup_failure.starts_with('@'))
                || (directory.starts_with("sdks/") && npm_downgrade);
            assert_eq!(output.status.success(), !should_fail, "{file}: {output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            if should_fail {
                assert!(stdout.is_empty(), "publication must not start: {stdout}");
                assert!(fs::read_to_string(&env_file).unwrap().is_empty());
                continue;
            }
            let latest = expected == "latest";
            if job.is_empty() {
                assert_eq!(
                    fs::read_to_string(&env_file).unwrap(),
                    format!("PUBLISH_LATEST_TAG={latest}\n")
                );
            } else if job == "preparation" {
                assert!(
                    stdout
                        .lines()
                        .any(|line| line == format!("--latest={latest}")),
                    "{stdout}"
                );
            } else {
                let args: Vec<_> = stdout.lines().collect();
                let channels: Vec<_> = args
                    .windows(2)
                    .filter(|pair| pair[0] == "--tag")
                    .map(|pair| pair[1])
                    .collect();
                let expected_channels = if directory == "sdks/ts" {
                    vec![
                        expected,
                        if expected == "release-1.6" {
                            "latest"
                        } else {
                            expected
                        },
                        expected,
                    ]
                } else {
                    vec![expected]
                };
                assert_eq!(channels, expected_channels);
            }
        }
    }
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
    let baseline = git(&repo, &["rev-parse", "HEAD"]);
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
    git(&repo, &["checkout", "-b", "9.8.x", &baseline]);
    git(&repo, &["merge", "--squash", "release"]);
    git(&repo, &["commit", "-m", "squash maintenance release PR"]);
    let maintenance_merged = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "later maintenance"],
    );
    git(&repo, &["branch", "9.7.x"]);
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
    let mut maintenance_pull = pull.clone();
    maintenance_pull["base"]["ref"] = json!("9.8.x");
    maintenance_pull["merge_commit_sha"] = json!(maintenance_merged);
    validate(json!([maintenance_pull]), true);
    let mut wrong_line = maintenance_pull.clone();
    wrong_line["base"]["ref"] = json!("9.7.x");
    validate(json!([wrong_line]), false);
    let mut wrong_target = maintenance_pull.clone();
    wrong_target["base"]["ref"] = json!("main");
    validate(json!([wrong_target]), false);
    let mut unreachable = maintenance_pull.clone();
    unreachable["merge_commit_sha"] = json!(merged);
    validate(json!([unreachable]), false);
    fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
    validate(json!([pull]), false);
    validate(json!([maintenance_pull]), false);
    fs::remove_file(repo.join("dirty.txt")).unwrap();
    git(&repo, &["tag", "--force", "v9.8.7-rc1", &merged]);
    validate(json!([pull]), false);
    git(&repo, &["checkout", "--detach", &merged]);
    validate(json!([pull]), false);
}

#[test]
fn release_publication_requires_successful_preparation_of_the_exact_tag() {
    let fixture = TempDir::new().unwrap();
    let repo = fixture.path();
    fs::copy(
        crate::workspace_path().join("Makefile.toml"),
        repo.join("Makefile.toml"),
    )
    .unwrap();
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    git(repo, &["init"]);
    git(repo, &["config", "user.name", "release-test"]);
    git(
        repo,
        &["config", "user.email", "release-test@example.invalid"],
    );
    git(repo, &["config", "commit.gpgsign", "false"]);
    git(repo, &["config", "core.hooksPath", "/dev/null"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "source"]);
    let head = git(repo, &["rev-parse", "HEAD"]);
    let bin = repo.join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, "#!/bin/sh\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = --jq ]; then exec jq -r \"$2\" \"$RELEASE_TEST_RUNS\"; fi\n  shift\ndone\nexit 1\n").unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let run = json!({
        "id": 42, "head_sha": head, "head_branch": "v9.8.7-rc1",
        "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
        "updated_at": "2026-10-18T12:00:00Z"
    });
    let validate = |runs: serde_json::Value, expected| {
        let response = repo.join("runs.json");
        fs::write(&response, json!({"workflow_runs": runs}).to_string()).unwrap();
        let output = Command::new("cargo")
            .current_dir(repo)
            .args([
                "make",
                "--disable-check-for-updates",
                "--no-workspace",
                "check-golem-release-preparation",
            ])
            .env_remove("CI")
            .env("PATH", &path)
            .env("RELEASE_TEST_RUNS", response)
            .env("BUILTIN_VERSION", "9.8.7-rc1")
            .env("RELEASE_PUBLISHED_AT", "2026-10-18T12:00:01Z")
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    validate(json!([run]), true);
    validate(json!([]), false);
    for (field, value) in [
        ("head_sha", json!("other-source")),
        ("head_branch", json!("release/9.8.7-rc1")),
        ("event", json!("push")),
        ("status", json!("in_progress")),
        ("conclusion", json!("failure")),
        ("updated_at", json!("2026-10-18T12:00:02Z")),
    ] {
        let mut rejected = run.clone();
        rejected[field] = value;
        validate(json!([rejected]), false);
    }
}

#[test]
fn builtin_packaging_failure_precedes_every_publication() {
    let fixture = TempDir::new().unwrap();
    let repo = fixture.path();
    let makefile = fs::read_to_string(crate::workspace_path().join("Makefile.toml")).unwrap();
    fs::write(repo.join("Release.toml"), makefile).unwrap();
    // Replace source validation and expensive compilation, not packaging or publication.
    fs::write(repo.join("Makefile.toml"), "extend = 'Release.toml'\n[tasks.validate-builtin-release]\nclear = true\nscript_runner = '@duckscript'\nscript = 'echo validated'\n[tasks.ensure-builtin-artifacts]\nclear = true\nscript_runner = '@duckscript'\nscript = '''\nmkdir plugins\nmkdir builtin-tools\nwritefile plugins/otlp-exporter.wasm plugin\nwritefile builtin-tools/filesystem-tools.wasm tools\n'''\n").unwrap();
    fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    fs::write(repo.join("LICENSE"), "test license").unwrap();
    fs::write(
        repo.join(".gitignore"),
        "bin/\nplugins/\nbuiltin-tools/\ntarget/\nvalidated\n",
    )
    .unwrap();
    fs::write(repo.join("builtin-artifacts.json"), json!({
        "otlp_exporter": {"component": "otlp-exporter", "path": "plugins/otlp-exporter.wasm"},
        "filesystem_tools": {"component": "filesystem-tools", "path": "builtin-tools/filesystem-tools.wasm"}
    }).to_string()).unwrap();
    git(repo, &["init"]);
    git(repo, &["config", "user.name", "release-test"]);
    git(
        repo,
        &["config", "user.email", "release-test@example.invalid"],
    );
    git(repo, &["config", "commit.gpgsign", "false"]);
    git(repo, &["config", "core.hooksPath", "/dev/null"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "packaging source"]);
    let bin = repo.join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "gh",
            "#!/bin/sh\necho 'Unexpected publication' >&2\nexit 99\n",
        ),
        (
            "wasm-tools",
            "#!/bin/sh\nif [ -f validated ] && [ -z \"$PACKAGING_SUCCEEDS\" ]; then echo 'second package failed' >&2; exit 1; fi\ntouch validated\n",
        ),
    ] {
        let file = bin.join(name);
        fs::write(&file, script).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let target = repo.join("target");
    let output = Command::new("cargo")
        .current_dir(repo)
        .args([
            "make",
            "--disable-check-for-updates",
            "--no-workspace",
            "publish-builtin-artifacts",
        ])
        .env_remove("CI")
        .env_remove("BUILTIN_DRY_RUN")
        .env("PATH", &path)
        .env("CARGO_TARGET_DIR", &target)
        .env("BUILTIN_VERSION", "9.8.7-rc1")
        .output()
        .unwrap();
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{log}");
    assert!(log.contains("second package failed"), "{log}");
    assert!(!log.contains("Unexpected publication"), "{log}");
    assert!(!repo.join("builtin-artifacts.release.json").exists());
    let packages = fs::read_dir(target.join("builtin-artifact-release")).unwrap();
    assert_eq!(
        packages
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .join("release-notes.md")
                .exists())
            .count(),
        1,
        "the first package must finish before the injected second failure: {log}"
    );

    // Exercise the same production upload code against a local release store.
    let gh = repo.join("bin/gh");
    fs::write(&gh, r#"#!/bin/sh
set -eu
command=$2
tag=$3
shift 3
case "$command" in
  view)
    if [ "$tag" = v9.8.7-rc1 ]; then echo '{"assets":[]}'; exit 0; fi
    [ -d "public/$tag" ] || exit 1
    if [ "$#" -eq 2 ]; then exit 0; fi
    find "public/$tag" -type f -exec basename {} \; | jq -R . | jq -s '{assets: map({name: .})}'
    ;;
  create)
    [ -f "$RELEASE_TEST_TARGET/builtin-artifact-release/otlp-exporter-v9.8.7-rc1/release-notes.md" ]
    [ -f "$RELEASE_TEST_TARGET/builtin-artifact-release/filesystem-tools-v9.8.7-rc1/release-notes.md" ]
    mkdir -p "public/$tag"
    ;;
  upload) mkdir -p "public/$tag"; cp "$1" "public/$tag/" ;;
  download)
    shift 2
    pattern=$2
    dir=$4
    cp "public/$tag/$pattern" "$dir/"
    ;;
  *) exit 99 ;;
esac
"#).unwrap();
    let curl = repo.join("bin/curl");
    fs::write(
        &curl,
        r#"#!/bin/sh
set -eu
shift 5
output=$1
url=$2
relative=${url#*/releases/download/}
cp "public/$relative" "$output"
"#,
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        repo.join(".gitignore"),
        "bin/\nplugins/\nbuiltin-tools/\ntarget/\nvalidated\npublic/\n",
    )
    .unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-m", "ignore local release store"]);
    let output = Command::new("cargo")
        .current_dir(repo)
        .args([
            "make",
            "--disable-check-for-updates",
            "--no-workspace",
            "publish-builtin-artifacts",
        ])
        .env_remove("CI")
        .env_remove("BUILTIN_DRY_RUN")
        .env("PATH", &path)
        .env("CARGO_TARGET_DIR", &target)
        .env("RELEASE_TEST_TARGET", &target)
        .env("PACKAGING_SUCCEEDS", "1")
        .env("BUILTIN_VERSION", "9.8.7-rc1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(repo.join("public/v9.8.7-rc1/builtin-artifacts.release.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["artifacts"].as_object().unwrap().len(), 2);
    for (id, component) in [
        ("otlp_exporter", "otlp-exporter"),
        ("filesystem_tools", "filesystem-tools"),
    ] {
        let bytes =
            fs::read(repo.join(format!("public/{component}-v9.8.7-rc1/{component}.wasm"))).unwrap();
        assert_eq!(
            bytes,
            if id == "otlp_exporter" {
                b"plugin".as_slice()
            } else {
                b"tools".as_slice()
            }
        );
        assert_eq!(manifest["artifacts"][id]["version"], "9.8.7-rc1");
    }
}
