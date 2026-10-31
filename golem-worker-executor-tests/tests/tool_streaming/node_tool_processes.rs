// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::*;
use std::time::Instant;
use test_r::test;

#[test]
#[timeout("3m")]
async fn builtin_node_and_npm_dispatch_bound_tools(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_builtin_tool_dispatch(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        false,
    )
    .await
}

/// Join asynchronous bound-tool calls before an npm lifecycle child finishes.
#[test]
#[timeout("3m")]
async fn builtin_npm_async_node_child_dispatch_bound_tools(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_builtin_tool_dispatch(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        true,
    )
    .await
}

async fn exercise_builtin_tool_dispatch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    streaming: &PrecompiledComponent,
    javascript_tools: &PrecompiledComponent,
    bash: &PrecompiledComponent,
    async_lifecycle: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let mut native_streaming = native_streaming_tool_metadata();
    native_streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            native_tool_metadata: Some(native_streaming.clone()),
            configure: Some(Arc::new(|config| {
                config.retry.max_attempts = 0;
            })),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let mut deployment =
        builtin_tool_deployment(&executor, deps, &context, javascript_tools, streaming, bash)
            .await?;
    let native = native_deployment_state(
        context.account_id,
        "ToolStreamingCaller",
        native_streaming,
        native_test_tool_metadata(),
    );
    deployment.registered_tools.extend(native.registered_tools);
    for (owner, bindings) in native.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        for binding in bindings.values_mut() {
            binding.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let synchronous = r#"
const assert = require('node:assert/strict');
const { spawnSync, execFile } = require('node:child_process');
const args = ['dual-reconstruct', 'redaction-terminals'];
const shell = spawnSync('printf shell-out; printf shell-err >&2; exit 9', [], {
    shell: true, encoding: 'utf8',
});
assert.equal(shell.error, undefined);
assert.equal(shell.status, 9);
assert.equal(shell.stdout, 'shell-out');
assert.equal(shell.stderr, 'shell-err');

const timed = spawnSync('native-streaming', ['run', 'wait-cancel'], {
    encoding: 'utf8', timeout: 1000, killSignal: 'SIGINT',
});
assert.equal(timed.error.code, 'ETIMEDOUT');
assert.equal(timed.status, null);
assert.equal(timed.signal, 'SIGINT');
assert.equal(timed.stdout, 'native:started');
assert.ok(timed.stderr.startsWith('diagnostic:started'));

const declared = spawnSync('streaming', ['run', 'declared-error'], { encoding: 'utf8' });
assert.equal(declared.error, undefined);
assert.equal(declared.status, 7);
assert.equal(declared.stdout, 'marker:');
assert.ok(declared.stderr.length > 0);

const trapped = spawnSync('streaming', ['run', 'trap'], { encoding: 'utf8' });
assert.equal(trapped.error.code, 'EIO');
assert.equal(trapped.status, null);
assert.equal(trapped.stdout, 'marker:');
assert.ok(trapped.stderr.length > 0);

const sync = spawnSync('streaming', args, { encoding: 'utf8' });
assert.equal(sync.error, undefined);
assert.equal(sync.status, 0);
assert.equal(sync.stdout, 'marker:');
assert.equal(sync.stderr, 'stderr-visible');
"#;
    let script = format!(
        "{synchronous}{}",
        r#"
await new Promise((resolve, reject) => execFile('streaming', args, { encoding: 'utf8' }, (error, stdout, stderr) => {
    if (error) return reject(error);
    try {
        assert.equal(stdout, 'marker:');
        assert.equal(stderr, 'stderr-visible');
        resolve();
    } catch (error) { reject(error); }
}));
console.log('bound tools passed');
"#
    );
    let agent = "builtin-node-tool-dispatch";
    let node = invoke_cli_tool(
        &executor,
        &caller_component,
        agent,
        "node",
        "/workspace",
        vec!["-e", &script],
    )
    .await?;
    assert_eq!(node.exit_code, 0, "{node:?}");
    assert_eq!(node.stdout, b"bound tools passed\n");
    assert!(node.stderr.is_empty(), "{node:?}");

    let lifecycle = if async_lifecycle {
        script
    } else {
        format!("{synchronous} console.log('bound tools passed');")
    };
    let setup = format!(
        "require('node:fs').writeFileSync('/workspace/dispatch.cjs', {}); require('node:fs').writeFileSync('/workspace/package.json', JSON.stringify({{name: 'bound-tool-dispatch', scripts: {{verify: 'node dispatch.cjs'}}}}));",
        serde_json::to_string(&format!(
            "(async () => {{ {lifecycle} }})().catch(error => {{ console.error(error); process.exitCode = 1; }});"
        ))?,
    );
    let setup = invoke_cli_tool(
        &executor,
        &caller_component,
        agent,
        "node",
        "/workspace",
        vec!["-e", &setup],
    )
    .await?;
    assert_eq!(setup.exit_code, 0, "{setup:?}");
    assert!(setup.stdout.is_empty());
    assert!(setup.stderr.is_empty(), "{setup:?}");
    let npm = invoke_cli_tool(
        &executor,
        &caller_component,
        agent,
        "npm",
        "/workspace",
        vec!["run", "verify", "--silent"],
    )
    .await?;
    assert_eq!(npm.exit_code, 0, "{npm:?}");
    assert_eq!(npm.stdout, b"bound tools passed\n");
    assert!(npm.stderr.is_empty(), "{npm:?}");
    Ok(())
}

/// Binds the built-in JavaScript tools, the streaming provider and Bash to the caller.
async fn builtin_tool_deployment(
    executor: &TestWorkerExecutor,
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    javascript_tools: &PrecompiledComponent,
    streaming: &PrecompiledComponent,
    bash: &PrecompiledComponent,
) -> anyhow::Result<ToolDeploymentState> {
    let mut deployment: Option<ToolDeploymentState> = None;
    for (component, package) in [
        (javascript_tools, "golem:javascript-tools"),
        (streaming, "golem-it:tool-streaming-rust-provider"),
        (bash, "golem:bash"),
    ] {
        let provider = executor
            .component_dep(&context.default_environment_id, component)
            .store()
            .await?;
        let metadata = extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", component.wasm_name)),
            false,
            true,
        )
        .await?;
        let next = deployment_state(
            context.account_id,
            provider.id,
            provider.revision,
            package,
            "ToolStreamingCaller",
            metadata.tools,
        );
        if let Some(current) = deployment.as_mut() {
            current.registered_tools.extend(next.registered_tools);
            for (owner, bindings) in next.tool_bindings {
                current
                    .tool_bindings
                    .entry(owner)
                    .or_default()
                    .extend(bindings);
            }
        } else {
            deployment = Some(next);
        }
    }
    Ok(deployment.unwrap())
}

/// Crash the owner while built-in `node` holds a bound-tool call with partial output on
/// both channels.
#[test]
#[timeout("5m")]
async fn builtin_node_crash_during_pending_tool_output(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    crash_during_pending_tool_attachments(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        false,
        false,
    )
    .await
}

/// Crash the owner while an npm lifecycle script's local Node child holds a bound-tool call
/// with partial output on both channels.
#[test]
#[timeout("5m")]
async fn builtin_npm_lifecycle_crash_during_pending_tool_output(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    crash_during_pending_tool_attachments(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        true,
        false,
    )
    .await
}

/// Crash the owner while built-in `node` also holds a second bound-tool call with an open
/// stdin.
///
/// Four of nine isolated runs hung after the crash: replay reached the checkpoint again
/// without repeating the effect and the held call completed after release, but the script's
/// final stdin write and end never reached the provider. Each hung run logged `terminal guard
/// abandoned during runtime teardown; leaving the end markerless` for the pending
/// `tool-stdin-closed::wait` Start at the crash, and its `End` was the first entry recorded
/// after recovery. No passing run logged it.
#[test]
#[ignore = "intermittently hangs: crash teardown can settle a pending tool stdin-closed wait, and replay then treats the open stdin as closed"]
#[timeout("5m")]
async fn builtin_node_crash_during_open_tool_stdin(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    crash_during_pending_tool_attachments(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        false,
        true,
    )
    .await
}

/// Run the open-stdin crash cut in an npm lifecycle script's local Node child.
///
/// Without the crash this passes. With it, four of five isolated runs hung in the same way and
/// with the same log. A local Node child started by built-in `node` with `execFile` instead of
/// npm also hung, so the hang is not specific to npm.
#[test]
#[ignore = "intermittently hangs: crash teardown can settle a pending tool stdin-closed wait, and replay then treats the open stdin as closed"]
#[timeout("5m")]
async fn builtin_npm_lifecycle_crash_during_open_tool_stdin(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    #[tagged_as("javascript_tools")] javascript_tools: &PrecompiledComponent,
    #[tagged_as("bash")] bash: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    crash_during_pending_tool_attachments(
        last_unique_id,
        deps,
        caller,
        streaming,
        javascript_tools,
        bash,
        true,
        true,
    )
    .await
}

const CRASH_CUT_PRELUDE: &str = r#"
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const capture = child => {
    const output = { stdout: [], stderr: [] };
    let progress = () => {};
    for (const name of ['stdout', 'stderr'])
        child[name].on('data', bytes => { output[name].push(...bytes); progress(); });
    const done = new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('close', (status, signal) => resolve({ status, signal, ...output }));
    });
    const stdout = length => new Promise(resolve => {
        progress = () => output.stdout.length >= length && resolve();
        progress();
    });
    return { done, stdout };
};
const hex = bytes => Buffer.from(bytes).toString('hex');
"#;

/// `marker-echo` writes its marker, then echoes stdin until it ends. The script writes one
/// stdin chunk and waits for its echo, so that call holds an open stdin and stdout while the
/// script awaits the held call.
const OPEN_STDIN_CRASH_CUT: &str = r#"
const echo = spawn('streaming', ['run', 'marker-echo']);
const echoed = capture(echo);
await echoed.stdout(7);
echo.stdin.write(Uint8Array.from([0, 255, 1]));
await echoed.stdout(10);
const held = await capture(spawn('streaming', ['dual-reconstruct', 'after-both-partial'])).done;
echo.stdin.end(Uint8Array.from([128, 13]));
const tail = await echoed.done;
assert.deepEqual([held.status, held.signal, tail.status, tail.signal], [0, null, 0, null]);
console.log(`held:${Buffer.from(held.stdout)}|${Buffer.from(held.stderr)} echo:${hex(tail.stdout)}|${hex(tail.stderr)}`);
"#;
const OPEN_STDIN_CRASH_CUT_OUTPUT: &[u8] =
    b"held:stdout-firststdout-last|stderr-firststderr-last echo:6d61726b65723a00ff01800d|\n";

/// `dual-reconstruct` records one HTTP effect, writes partial stdout and stderr and holds at
/// the `after-both-partial` crash checkpoint before either output terminal.
const OUTPUT_CRASH_CUT: &str = r#"
const held = await capture(spawn('streaming', ['dual-reconstruct', 'after-both-partial'])).done;
assert.deepEqual([held.status, held.signal], [0, null]);
console.log(`held:${Buffer.from(held.stdout)}|${Buffer.from(held.stderr)}`);
"#;
const OUTPUT_CRASH_CUT_OUTPUT: &[u8] = b"held:stdout-firststdout-last|stderr-firststderr-last\n";

const CRASH_CUT_CHECKPOINT: &str = "after-both-partial";
const CRASH_CUT_EFFECT: &str = "dual-reconstruct-after-both-partial";

const FOLLOW_UP_SCRIPT: &str = r#"
const { spawnSync } = require('node:child_process');
const result = spawnSync('streaming', ['dual-reconstruct', 'redaction-terminals'], { encoding: 'utf8' });
console.log(`follow-up:${result.status}:${result.stdout}|${result.stderr}`);
"#;
const FOLLOW_UP_OUTPUT: &[u8] = b"follow-up:0:marker:|stderr-visible\n";

async fn crash_during_pending_tool_attachments(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    streaming: &PrecompiledComponent,
    javascript_tools: &PrecompiledComponent,
    bash: &PrecompiledComponent,
    lifecycle: bool,
    open_stdin: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            configure: Some(Arc::new(|config| {
                config.retry.max_attempts = 0;
                config.oplog.default_snapshotting =
                    golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
            })),
            ..Default::default()
        }
        .with_agent_sockets(),
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let mut deployment =
        builtin_tool_deployment(&executor, deps, &context, javascript_tools, streaming, bash)
            .await?;
    // The JavaScript tools need the owner filesystem. A capable `streaming` call defers its
    // body, so it would not echo while the script keeps its stdin open. The provider stays
    // incapable, as in the probe deployment.
    let streaming_tool = ToolName::try_from("streaming").expect("valid streaming tool name");
    for bindings in deployment.tool_bindings.values_mut() {
        for (name, binding) in bindings.iter_mut() {
            if *name != streaming_tool {
                binding.filesystem_access = ToolFilesystemAccess::Allowed;
            }
        }
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoints) =
        start_crash_checkpoint_server().await;
    let (effect_port, mut effects, effect_server) =
        middleware_acceptance::start_probe_effect_server().await;
    let name = format!(
        "{}-tool-process-crash-cut-{}",
        if lifecycle { "npm" } else { "node" },
        if open_stdin { "stdin" } else { "output" }
    );
    let agent = agent_id!("ToolStreamingCaller", name.clone());
    let worker = executor
        .start_agent_with(
            &caller_component.id,
            agent.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker);

    let (body, expected) = if open_stdin {
        (OPEN_STDIN_CRASH_CUT, OPEN_STDIN_CRASH_CUT_OUTPUT)
    } else {
        (OUTPUT_CRASH_CUT, OUTPUT_CRASH_CUT_OUTPUT)
    };
    let script = format!("{CRASH_CUT_PRELUDE}{body}");
    let (tool, args) = if lifecycle {
        let module = |body: &str| {
            serde_json::to_string(&format!(
                "(async () => {{ {body} }})().catch(error => {{ console.error(error); process.exitCode = 1; }});"
            ))
        };
        let setup = format!(
            "const fs = require('node:fs'); fs.writeFileSync('/workspace/crash-cut.cjs', {}); fs.writeFileSync('/workspace/follow-up.cjs', {}); fs.writeFileSync('/workspace/package.json', JSON.stringify({{name: 'tool-process-crash-cut', scripts: {{'crash-cut': 'node crash-cut.cjs', 'follow-up': 'node follow-up.cjs'}}}}));",
            module(&script)?,
            module(FOLLOW_UP_SCRIPT)?,
        );
        let setup = invoke_cli_tool(
            &executor,
            &caller_component,
            &name,
            "node",
            "/workspace",
            vec!["-e", &setup],
        )
        .await?;
        assert_eq!(setup.exit_code, 0, "{setup:?}");
        assert!(
            setup.stdout.is_empty() && setup.stderr.is_empty(),
            "{setup:?}"
        );
        ("npm", vec!["run", "crash-cut", "--silent"])
    } else {
        ("node", vec!["-e", script.as_str()])
    };
    let key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &caller_component,
            &agent,
            &key,
            "builtin_cli",
            data_value!(tool, "/workspace", args.clone()),
        )
        .await?;

    let Ok(effect) = tokio::time::timeout(Duration::from_secs(60), effects.recv()).await else {
        let metadata = executor.get_worker_metadata(&worker).await?;
        let active = executor.active_entity_metadata(&owner).await;
        anyhow::bail!(
            "the held provider did not record its effect; status {:?}, last error {:?}; active: {active:#?}",
            metadata.status,
            metadata.last_error
        );
    };
    assert_eq!(effect.as_deref(), Some(CRASH_CUT_EFFECT));
    let original = next_crash_checkpoint(&mut checkpoints, CRASH_CUT_CHECKPOINT).await?;
    let recorded_before =
        persist_pending_tool_attachments(&executor, &worker, &owner, open_stdin).await?;

    executor.simulated_crash(&worker).await?;
    drop(original.release);
    // Replay re-executes the JavaScript process and its calls up to the same checkpoint. The
    // interrupted gate read fails there, so the provider announces the checkpoint again.
    let replayed = next_crash_checkpoint(&mut checkpoints, CRASH_CUT_CHECKPOINT).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), effects.recv())
            .await
            .is_err(),
        "replay repeated the recorded provider effect"
    );
    replayed
        .release
        .send(())
        .map_err(|_| anyhow::anyhow!("the replayed checkpoint gate was dropped"))?;

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        executor.invoke_and_await_agent_with_key(
            &caller_component,
            &agent,
            &key,
            "builtin_cli",
            data_value!(tool, "/workspace", args),
        ),
    )
    .await;
    let Ok(result) = result else {
        let metadata = executor.get_worker_metadata(&worker).await?;
        let active = executor.active_entity_metadata(&owner).await;
        let oplog = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        anyhow::bail!(
            "the recovered invocation did not complete; status {:?}, last error {:?}; active: {active:#?}; oplog:\n{}",
            metadata.status,
            metadata.last_error,
            describe_history(&oplog)
        );
    };
    let result: CliToolEvidence = result?.into_typed()?;
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(expected)
    );
    assert!(result.stderr.is_empty(), "{result:?}");
    // The effect End was committed before the crash, so replay must not resend it.
    assert!(
        effects.try_recv().is_err(),
        "unexpected additional live effect"
    );
    assert!(
        checkpoints.try_recv().is_err(),
        "the checkpoint was reached more than once after the crash"
    );

    let recorded_after = executor.stored_oplog(&worker).await;
    assert!(recorded_after.len() >= recorded_before.len());
    for (index, (before, after)) in recorded_before.iter().zip(&recorded_after).enumerate() {
        assert_eq!(
            before,
            after,
            "recorded prefix changed at entry {}",
            index + 1
        );
    }
    let history = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    assert_collector_history_settled(&history);
    assert_eq!(
        history
            .iter()
            .filter(|entry| matches!(
                &entry.entry,
                PublicOplogEntry::AgentInvocationStarted(started)
                    if matches!(
                        &started.invocation,
                        PublicAgentInvocation::AgentMethodInvocation(method)
                            if method.idempotency_key == key
                    )
            ))
            .count(),
        1,
        "recovery must complete the original invocation exactly once"
    );
    assert!(
        history
            .iter()
            .all(|entry| !matches!(entry.entry, PublicOplogEntry::Error(_))),
        "recovery must not record an owner error"
    );
    let metadata = executor.get_worker_metadata(&worker).await?;
    assert_eq!(metadata.retry_count, 0);
    assert_ne!(metadata.status, AgentStatus::Failed);

    let (tool, args) = if lifecycle {
        ("npm", vec!["run", "follow-up", "--silent"])
    } else {
        ("node", vec!["-e", FOLLOW_UP_SCRIPT])
    };
    let next = invoke_cli_tool(
        &executor,
        &caller_component,
        &name,
        tool,
        "/workspace",
        args,
    )
    .await?;
    assert_eq!(next.exit_code, 0, "after recovery: {next:?}");
    assert_eq!(
        String::from_utf8_lossy(&next.stdout),
        String::from_utf8_lossy(FOLLOW_UP_OUTPUT)
    );
    assert!(next.stderr.is_empty(), "after recovery: {next:?}");
    assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
    assert!(effects.try_recv().is_err() && checkpoints.try_recv().is_err());
    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}

/// Waits until the held calls expose their open attachments, commits the owner oplog and
/// checks that the provider effect is complete while the calls are still open. Returns the
/// stored oplog at that point.
async fn persist_pending_tool_attachments(
    executor: &TestWorkerExecutor,
    worker: &golem_common::model::AgentId,
    owner: &OwnedAgentId,
    open_stdin: bool,
) -> anyhow::Result<Vec<golem_common::model::oplog::OplogEntry>> {
    let open =
        |attachment: &Option<golem_worker_executor::durable_host::tool::ToolAttachmentMetadata>,
         bytes: u64| {
            attachment.as_ref().is_some_and(|attachment| {
                attachment.accepted_bytes >= bytes && !attachment.terminal_selected
            })
        };
    let mut last = None;
    let attachments =
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                last = executor.active_entity_metadata(owner).await;
                if let Some(active) = &last {
                    // `marker-echo` has echoed one stdin chunk and waits for the rest.
                    let echo = !open_stdin
                        || active.tool_operations.operations.iter().any(|operation| {
                            open(&operation.stdin, 3) && open(&operation.stdout, 10)
                        });
                    // `dual-reconstruct` holds partial output on both channels.
                    let held = active.tool_operations.operations.iter().any(|operation| {
                        open(&operation.stdout, 12) && open(&operation.stderr, 12)
                    });
                    if echo && held {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    if attachments.is_err() {
        anyhow::bail!("the held calls did not expose open attachments: {last:#?}");
    }
    executor.commit_oplog(worker).await?;
    let oplog = executor.get_oplog(worker, OplogIndex::INITIAL).await?;
    let terminated = |start: OplogIndex| {
        oplog.iter().any(|entry| match &entry.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
    };
    let starts = |function_name: &str| {
        oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == function_name)
            })
            .map(|entry| entry.oplog_index)
            .collect::<Vec<_>>()
    };
    // The effect and the checkpoint announcement have both completed.
    let sends = starts("http::client::send");
    anyhow::ensure!(
        sends.len() == 2 && sends.iter().all(|send| terminated(*send)),
        "expected two completed HTTP sends at the checkpoint:\n{}",
        describe_history(&oplog)
    );
    // The JavaScript tool and the held provider calls are still open.
    let expected = 2 + usize::from(open_stdin);
    let open_entities = starts("golem::entity::invoke")
        .into_iter()
        .filter(|start| !terminated(*start))
        .count();
    anyhow::ensure!(
        open_entities == expected,
        "expected {expected} open entity invocations at the checkpoint, found {open_entities}:\n{}",
        describe_history(&oplog)
    );
    Ok(executor.stored_oplog(worker).await)
}

/// Exercise a guest-owned stdin pump, output drains and result wait through
/// ordinary tool imports. Build the fixture before running this test.
#[test]
#[ignore]
#[timeout("3m")]
async fn node_guest_tool_process_bridge(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_provider")] streaming: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let probe_path =
        std::path::PathBuf::from(std::env::var("GOLEM_NODE_TOOL_EXECUTOR_PROBE_WASM")?)
            .canonicalize()?;
    let cases = std::env::var("GOLEM_NODE_TOOL_STREAM_PROBE_CASES")?;
    let execution = std::env::var("GOLEM_NODE_TOOL_STREAM_PROBE_EXECUTION")
        .unwrap_or_else(|_| "all".to_string());
    let dispatch = std::env::var("GOLEM_NODE_TOOL_STREAM_PROBE_DISPATCH")
        .unwrap_or_else(|_| "public-api".to_string());
    anyhow::ensure!(matches!(dispatch.as_str(), "public-api" | "adapter"));
    anyhow::ensure!(matches!(
        execution.as_str(),
        "all"
            | "main-sync"
            | "main-async"
            | "main-both"
            | "main-overlap"
            | "main-cancel"
            | "main-native-cancel"
            | "main-sdk-overflow"
            | "fresh-sync"
            | "fresh-async"
            | "fresh-native-cancel"
            | "fresh-sdk-overflow"
            | "fresh-stream-control"
            | "child-sync"
            | "child-async"
    ));
    let replay_probe = std::env::var_os("GOLEM_NODE_TOOL_STREAM_REPLAY_PROBE").is_some();
    anyhow::ensure!(cases.split(',').all(|mode| matches!(
        mode,
        "streams-collect" | "streams-echo-collect" | "streams-pressure-collect"
    )));
    anyhow::ensure!(
        !matches!(
            execution.as_str(),
            "fresh-stream-control"
                | "main-overlap"
                | "main-cancel"
                | "main-native-cancel"
                | "fresh-native-cancel"
                | "fresh-sdk-overflow"
                | "main-sdk-overflow"
        ) || cases == "streams-collect"
    );
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        configure: Some(Arc::new(|config| {
            config.retry.max_attempts = 0;
            config.limits.max_tool_attachment_bytes = 65536;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let probe = executor
        .component(
            &context.default_environment_id,
            &probe_path.with_extension("").to_string_lossy(),
        )
        .store()
        .await?;
    let mut deployment = deployment_state(
        context.account_id,
        probe.id,
        probe.revision,
        "node-tool-processes-probe",
        "ToolStreamingCaller",
        extract_component_metadata(&probe_path, false, true)
            .await?
            .tools,
    );
    let provider = executor
        .component_dep(&context.default_environment_id, streaming)
        .store()
        .await?;
    let provider_deployment = deployment_state(
        context.account_id,
        provider.id,
        provider.revision,
        "golem-it:tool-streaming-rust-provider",
        "ToolStreamingCaller",
        extract_component_metadata(
            &deps
                .component_directory
                .join(format!("{}.wasm", streaming.wasm_name)),
            false,
            true,
        )
        .await?
        .tools,
    );
    deployment
        .registered_tools
        .extend(provider_deployment.registered_tools);
    for (owner, bindings) in provider_deployment.tool_bindings {
        deployment
            .tool_bindings
            .entry(owner)
            .or_default()
            .extend(bindings);
    }
    for bindings in deployment.tool_bindings.values_mut() {
        // The parent probe needs the owner filesystem, not its streaming child.
        bindings
            .get_mut(&ToolName::try_from("node-tool-probe").expect("valid probe tool name"))
            .unwrap()
            .filesystem_access = ToolFilesystemAccess::Allowed;
    }
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent = agent_id!("ToolStreamingCaller", "node-stream-scheduling");
    let worker = executor
        .start_agent(&caller_component.id, agent.clone())
        .await?;
    let mut effect_probe = if replay_probe {
        Some(middleware_acceptance::start_probe_effect_server().await)
    } else {
        None
    };
    let effect_url = effect_probe
        .as_ref()
        .map(|(port, _, _)| format!("http://127.0.0.1:{port}"));
    for mode in cases.split(',') {
        println!("node-tool-stream starting mode={mode} execution={execution} dispatch={dispatch}");
        let started = Instant::now();
        let args = vec![
            mode,
            effect_url.as_deref().unwrap_or(""),
            execution.as_str(),
            dispatch.as_str(),
        ];
        let label = if execution == "all" {
            mode.to_string()
        } else {
            format!("{mode}[{execution}]")
        };
        let label = if dispatch == "adapter" {
            format!("{label}[adapter]")
        } else {
            label
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            invoke_cli_tool(
                &executor,
                &caller_component,
                "node-stream-scheduling",
                "node-tool-probe",
                "/workspace",
                args,
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("guest bridge timed out: {mode}"))??;
        assert_eq!(result.exit_code, 0, "{mode}: {result:?}");
        assert!(result.stderr.is_empty(), "{mode}: {result:?}");
        if mode == "streams-pressure-collect" || execution.ends_with("sdk-overflow") {
            assert_eq!(
                String::from_utf8(result.stdout)?,
                format!("{label}:overflow:4096\n")
            );
            println!(
                "node-tool-stream completed mode={mode} elapsed_us={}",
                started.elapsed().as_micros()
            );
            continue;
        }
        let (bytes, checksum, stderr_bytes, stderr_checksum) = if mode == "streams-echo-collect" {
            (8, 661, 0, 0)
        } else {
            (7, 700, 14, 1455)
        };
        assert_eq!(
            String::from_utf8(result.stdout)?,
            format!("{label}:{bytes}:{checksum}:{stderr_bytes}:{stderr_checksum}\n")
        );
        println!(
            "node-tool-stream completed mode={mode} elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    if let Some((_, effects, server)) = effect_probe.as_mut() {
        for mode in cases.split(',') {
            for runtime in if mode.contains("echo") || execution.starts_with("main-") {
                ["main"].as_slice()
            } else {
                ["main", "job"].as_slice()
            } {
                let effect = tokio::time::timeout(Duration::from_secs(5), effects.recv()).await?;
                assert_eq!(effect, Some(format!("{mode}-{runtime}")));
            }
        }
        assert!(
            effects.try_recv().is_err(),
            "unexpected additional live effect"
        );
        let before = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        assert_collector_history_settled(&before);
        let recorded_before = executor.stored_oplog(&worker).await;
        server.abort();
        let _ = server.await;
        drop(executor);
        let executor = start_with_overrides(deps, &context, overrides).await?;
        let replayed: String = tokio::time::timeout(
            Duration::from_secs(30),
            executor.invoke_and_await_agent(
                &caller_component,
                &agent,
                "replay_probe",
                data_value!(),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("collector reconstruction timed out"))??
        .into_typed()?;
        assert_eq!(replayed, "replayed");
        assert!(
            effects.try_recv().is_err(),
            "completed replay repeated an external effect"
        );
        let after = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
        // Public span attributes come from maps and have no display order.
        // Compare the authoritative records, not that presentation order.
        let recorded_after = executor.stored_oplog(&worker).await;
        assert!(recorded_after.len() >= recorded_before.len());
        for (index, (before, after)) in recorded_before.iter().zip(&recorded_after).enumerate() {
            assert_eq!(
                before,
                after,
                "recorded prefix changed at entry {}",
                index + 1
            );
        }
        assert_collector_history_settled(&after);
        let next = invoke_cli_tool(
            &executor,
            &caller_component,
            "node-stream-scheduling",
            "node-tool-probe",
            "/workspace",
            vec!["streams-collect"],
        )
        .await?;
        assert_eq!(next.exit_code, 0, "after restart: {next:?}");
        assert_eq!(next.stdout, b"streams-collect:7:700:14:1455\n");
        assert!(next.stderr.is_empty());
        assert_collector_history_settled(&executor.get_oplog(&worker, OplogIndex::INITIAL).await?);
        println!("node-tool-stream reconstruction completed with the effect server stopped");
    }
    Ok(())
}

fn describe_history(history: &[PublicOplogEntryWithIndex]) -> String {
    history
        .iter()
        .map(|entry| {
            format!(
                "{} {}",
                entry.oplog_index,
                describe_public_entry(&entry.entry)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_collector_history_settled(history: &[PublicOplogEntryWithIndex]) {
    let last_finished = history
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("the owner has completed an invocation")
        .oplog_index;
    let mut open_invocation = None;
    for entry in history {
        match &entry.entry {
            PublicOplogEntry::AgentInvocationStarted(_) => {
                assert!(open_invocation.replace(entry.oplog_index).is_none());
            }
            PublicOplogEntry::AgentInvocationFinished(_) => {
                assert!(open_invocation.take().is_some());
            }
            PublicOplogEntry::Start(start) => {
                let terminals = history
                    .iter()
                    .filter(|terminal| match &terminal.entry {
                        PublicOplogEntry::End(end) => end.start_index == entry.oplog_index,
                        PublicOplogEntry::Cancelled(cancelled) => {
                            cancelled.start_index == entry.oplog_index
                        }
                        _ => false,
                    })
                    .count();
                if terminals == 0 {
                    assert!(
                        collector_call_abandoned_by_entity(history, entry),
                        "unsettled Start {} ({}) has no owning entity cancellation; parent={:?}",
                        entry.oplog_index,
                        start.function_name,
                        start.parent_start_index,
                    );
                } else {
                    assert_eq!(
                        terminals,
                        1,
                        "duplicated terminal for Start {} ({}): {:?}",
                        entry.oplog_index,
                        start.function_name,
                        history
                            .iter()
                            .filter_map(|terminal| match &terminal.entry {
                                PublicOplogEntry::End(end)
                                    if end.start_index == entry.oplog_index =>
                                    Some((terminal.oplog_index, "End")),
                                PublicOplogEntry::Cancelled(cancelled)
                                    if cancelled.start_index == entry.oplog_index =>
                                    Some((terminal.oplog_index, "Cancelled")),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                    );
                }
                assert!(entry.oplog_index < last_finished);
            }
            PublicOplogEntry::End(_) | PublicOplogEntry::Cancelled(_) => {
                assert!(entry.oplog_index < last_finished);
            }
            _ => {}
        }
    }
    assert!(open_invocation.is_none());
}

fn collector_call_abandoned_by_entity(
    history: &[PublicOplogEntryWithIndex],
    call: &PublicOplogEntryWithIndex,
) -> bool {
    let PublicOplogEntry::Start(start) = &call.entry else {
        return false;
    };
    if !matches!(
        start.function_name.as_str(),
        "golem::tool::streams::tool-output-writer::write" | "clocks::monotonic-clock::wait-for"
    ) || history.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::End(end) => end.start_index == call.oplog_index,
        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == call.oplog_index,
        _ => false,
    }) {
        return false;
    }
    let Some(parent) = start
        .parent_start_index
        .filter(|parent| *parent < call.oplog_index)
    else {
        return false;
    };
    if !history.iter().any(|entry| entry.oplog_index == parent &&
        matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke"))
    {
        return false;
    }
    // A cancelled body owns its abandoned subtree, including a write or timer
    // whose future was destroyed with the body. A normal End cannot own it.
    let mut terminals = history.iter().filter(|entry| match &entry.entry {
        PublicOplogEntry::End(end) => end.start_index == parent,
        PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == parent,
        _ => false,
    });
    matches!(terminals.next(), Some(entry) if entry.oplog_index > call.oplog_index &&
        matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == parent))
        && terminals.next().is_none()
}

#[test]
fn collector_abandonment_requires_the_owning_entity_cancellation() {
    use golem_common::model::oplog::PublicDurableFunctionType;
    use golem_common::model::oplog::public_oplog_entry::{CancelledParams, EndParams, StartParams};
    use golem_common::model::{Empty, Timestamp};

    let entry = |index, entry| PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(index),
        attribution: PublicOplogEntryAttribution::agent(),
        entry,
    };
    let start = |index, parent: Option<u64>, function: &str| {
        entry(
            index,
            PublicOplogEntry::Start(StartParams {
                timestamp: Timestamp::now_utc(),
                parent_start_index: parent.map(OplogIndex::from_u64),
                function_name: function.to_string(),
                invocation_id: None,
                observational_owner: None,
                request: None,
                durable_function_type: PublicDurableFunctionType::ReadLocal(Empty {}),
                span_started: None,
                denial: None,
            }),
        )
    };
    let cancel = |index, start| {
        entry(
            index,
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(start),
                partial: None,
                span_finished: None,
            }),
        )
    };
    let end = |index, start| {
        entry(
            index,
            PublicOplogEntry::End(EndParams {
                timestamp: Timestamp::now_utc(),
                start_index: OplogIndex::from_u64(start),
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            }),
        )
    };
    for function in [
        "golem::tool::streams::tool-output-writer::write",
        "clocks::monotonic-clock::wait-for",
    ] {
        let parent = start(1, None, "golem::entity::invoke");
        let call = start(2, Some(1), function);
        let history = vec![parent.clone(), call.clone(), cancel(3, 1)];
        assert!(collector_call_abandoned_by_entity(&history, &call));
        assert!(!collector_call_abandoned_by_entity(&history[..2], &call));
        assert!(!collector_call_abandoned_by_entity(&history[1..], &call));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), end(3, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 99)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 1), cancel(4, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[parent.clone(), call.clone(), cancel(3, 1), end(4, 2)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &[start(1, None, "unrelated"), call.clone(), cancel(3, 1)],
            &call
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(2, None, function)
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(4, Some(1), function)
        ));
        assert!(!collector_call_abandoned_by_entity(
            &history,
            &start(2, Some(1), "unrelated")
        ));
    }
}
