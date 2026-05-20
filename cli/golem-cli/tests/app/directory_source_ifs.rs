use crate::Tracing;
use crate::app::{TestContext, cmd, flag};

use golem_cli::fs;
use indoc::indoc;
use std::path::Path;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
async fn directory_source_ifs_deploys_and_updates_for_ts_agent_workspace(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-directory-source-ifs";

    let outputs = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "ts"])
        .await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    std::fs::remove_file(ctx.cwd_path_join(Path::new("src").join("counter-agent.ts"))).unwrap();

    fs::create_dir_all(ctx.cwd_path_join(Path::new("scratch").join("workspace"))).unwrap();
    fs::write_str(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join(".keep")),
        "seed\n",
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join("nested.txt")),
        "nested seed\n",
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join("removed.txt")),
        "removed seed\n",
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("ifs-probe-agent.ts")),
        indoc! {
            r#"
            import { BaseAgent, agent } from '@golemcloud/golem-ts-sdk';
            import * as fs from 'node:fs';

            @agent({})
            class IfsProbeAgent extends BaseAgent {
                constructor(name: string) {
                    super();
                }

                async probe(): Promise<string> {
                    fs.mkdirSync('/workspace/generated', { recursive: true });
                    fs.writeFileSync('/workspace/generated/output.txt', 'generated', 'utf8');

                    return JSON.stringify({
                        keep: fs.readFileSync('/workspace/.keep', 'utf8'),
                        nested: fs.readFileSync('/workspace/nested.txt', 'utf8'),
                        added: fs.existsSync('/workspace/added.txt') ? fs.readFileSync('/workspace/added.txt', 'utf8') : null,
                        removedExists: fs.existsSync('/workspace/removed.txt'),
                        generated: fs.readFileSync('/workspace/generated/output.txt', 'utf8'),
                    });
                }
            }

            @agent({})
            class SecondIfsProbeAgent extends BaseAgent {
                constructor(name: string) {
                    super();
                }

                async probe(): Promise<string> {
                    fs.mkdirSync('/workspace/generated', { recursive: true });
                    fs.writeFileSync('/workspace/generated/second-output.txt', 'second-generated', 'utf8');

                    return JSON.stringify({
                        keep: fs.readFileSync('/workspace/.keep', 'utf8'),
                        nested: fs.readFileSync('/workspace/nested.txt', 'utf8'),
                        added: fs.existsSync('/workspace/added.txt') ? fs.readFileSync('/workspace/added.txt', 'utf8') : null,
                        removedExists: fs.existsSync('/workspace/removed.txt'),
                        generated: fs.readFileSync('/workspace/generated/second-output.txt', 'utf8'),
                    });
                }
            }
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join(Path::new("src").join("main.ts")),
        indoc! {
            r#"
            export * from './ifs-probe-agent';
            "#
        },
    )
    .unwrap();

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"
            manifestVersion: 1.5.0

            app: test-app-directory-source-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-directory-source-ifs:ts-main:
                templates: ts

            agents:
              IfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace/.keep
                  targetPath: /workspace/.keep
                  permissions: read-write
        "},
    )
    .unwrap();

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("/workspace/.keep:"));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"
            manifestVersion: 1.5.0

            app: test-app-directory-source-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-directory-source-ifs:ts-main:
                templates: ts

            agents:
              IfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace
                  targetPath: /workspace
                  permissions: read-write
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("/workspace/nested.txt:"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "IfsProbeAgent(\"directory-source\")",
            "probe",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("keep"));
    assert!(outputs.stdout_contains("seed"));
    assert!(outputs.stdout_contains("nested"));
    assert!(outputs.stdout_contains("nested seed"));
    assert!(outputs.stdout_contains("generated"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            "files",
            "IfsProbeAgent(\"directory-source\")",
            "/workspace",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_row_with_cells(&[".keep", "file", "rw"]));
    assert!(outputs.stdout_contains_row_with_cells(&["nested.txt", "file", "rw"]));
    assert!(outputs.stdout_contains_row_with_cells(&["generated", "directory"]));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"
            manifestVersion: 1.5.0

            app: test-app-directory-source-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-directory-source-ifs:ts-main:
                templates: ts

            agents:
              IfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace
                  targetPath: /workspace
                  permissions: read-write
              SecondIfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace
                  targetPath: /workspace
                  permissions: read-write
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("SecondIfsProbeAgent"));
    assert!(outputs.stdout_contains("/workspace/.keep:"));
    assert!(outputs.stdout_contains("/workspace/nested.txt:"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "SecondIfsProbeAgent(\"directory-source\")",
            "probe",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("keep"));
    assert!(outputs.stdout_contains("seed"));
    assert!(outputs.stdout_contains("nested"));
    assert!(outputs.stdout_contains("nested seed"));
    assert!(outputs.stdout_contains("second-generated"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            "files",
            "SecondIfsProbeAgent(\"directory-source\")",
            "/workspace",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_row_with_cells(&[".keep", "file", "rw"]));
    assert!(outputs.stdout_contains_row_with_cells(&["nested.txt", "file", "rw"]));
    assert!(outputs.stdout_contains_row_with_cells(&["generated", "directory"]));

    fs::write_str(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join("nested.txt")),
        "updated nested seed\n",
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    for agent in ["IfsProbeAgent", "SecondIfsProbeAgent"] {
        let outputs = ctx
            .cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                &format!("{agent}(\"updated-content\")"),
                "probe",
            ])
            .await;
        assert!(outputs.success_or_dump());
        assert!(outputs.stdout_contains("updated nested seed"));
        assert!(outputs.stdout_contains("removedExists"));
        assert!(outputs.stdout_contains("true"));
    }

    fs::write_str(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join("added.txt")),
        "added seed\n",
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "IfsProbeAgent(\"added-file\")",
            "probe",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("added seed"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            "files",
            "IfsProbeAgent(\"added-file\")",
            "/workspace",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_row_with_cells(&["added.txt", "file", "rw"]));

    std::fs::remove_file(
        ctx.cwd_path_join(Path::new("scratch").join("workspace").join("removed.txt")),
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "IfsProbeAgent(\"removed-file\")",
            "probe",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains("removedExists"));
    assert!(outputs.stdout_contains("false"));

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            "files",
            "IfsProbeAgent(\"removed-file\")",
            "/workspace",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(!outputs.stdout_contains("removed.txt"));

    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        indoc! {"
            manifestVersion: 1.5.0

            app: test-app-directory-source-ifs

            environments:
              local:
                server: local
                componentPresets: debug

            components:
              test-app-directory-source-ifs:ts-main:
                templates: ts

            agents:
              IfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace
                  targetPath: /workspace
                  permissions: read-only
              SecondIfsProbeAgent:
                files:
                - sourcePath: ./scratch/workspace
                  targetPath: /workspace
                  permissions: read-write
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            "files",
            "IfsProbeAgent(\"readonly-directory\")",
            "/workspace",
        ])
        .await;
    assert!(outputs.success_or_dump());
    assert!(outputs.stdout_contains_row_with_cells(&[".keep", "file", "ro"]));
    assert!(outputs.stdout_contains_row_with_cells(&["nested.txt", "file", "ro"]));
    assert!(outputs.stdout_contains_row_with_cells(&["added.txt", "file", "ro"]));
}
