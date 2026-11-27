use super::{Output, TestContext, cmd, flag};
use anyhow::{Context, bail, ensure};
use golem_cli::model::agent::AgentGetView;
use golem_cli::model::agent::oplog::{AgentOplogEntryContent, AgentOplogEntryView};
use golem_cli::model::component::ComponentGetView;
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_cli::model::tool_deployment::DeployedToolView;
use golem_cli::sdk_overrides::SdkOverrides;
use golem_client::model::{NativeToolInvocationResponse, NativeToolResult};
use golem_common::model::invocation_session_public::PublicInvocationResult;
use golem_common::model::oplog::{PublicAgentInvocation, PublicOplogEntry};
use golem_common::model::{InvocationOwner, OwnedInvocationId};
use golem_common::schema::{
    TypedSchemaValue,
    render::{from_json_value, to_json_value},
};
use golem_size_analysis::breakdown::Bucket;
use golem_size_analysis::collect::{
    CollectInputs, collect, read_go_packages, read_module_map, to_json,
};
use golem_size_analysis::fixtures::{
    CallTarget, CheckedCall, Fixture, FixtureDescriptor, FixtureKind, HttpExpected, Sdk,
    ToolArguments, ToolExpected,
};
use heck::{ToKebabCase, ToSnakeCase};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use test_r::{test, timeout};

async fn command(ctx: &TestContext, args: Vec<String>) -> anyhow::Result<Output> {
    let output = tokio::time::timeout(Duration::from_secs(1200), ctx.cli(args))
        .await
        .context("CLI command exceeded its deadline")?;
    ensure!(output.success_or_dump(), "CLI command failed");
    Ok(output)
}

async fn selected_wasm(
    ctx: &TestContext,
    preset: &str,
    component: &str,
) -> anyhow::Result<PathBuf> {
    let output = command(
        ctx,
        vec![
            "component".into(),
            "manifest-trace".into(),
            "-P".into(),
            preset.into(),
            flag::FORMAT.into(),
            "json".into(),
        ],
    )
    .await?;
    let trace = output
        .stdout_json::<Value>()
        .into_iter()
        .find(|trace| trace["componentName"] == component)
        .context("missing component manifest trace")?;
    Ok(ctx.cwd_path_join(
        trace["properties"]["componentWasm"]["value"]
            .as_str()
            .context("missing resolved componentWasm")?,
    ))
}

async fn uploaded_wasm(
    ctx: &TestContext,
    preset: &str,
    component: &str,
) -> anyhow::Result<(PathBuf, Vec<u8>)> {
    let path = selected_wasm(ctx, preset, component).await?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("selected componentWasm {}", path.display()))?;
    ensure!(!bytes.is_empty(), "selected componentWasm is empty");
    let result = command(
        ctx,
        vec![
            "-P".into(),
            preset.into(),
            "component".into(),
            "get".into(),
            component.into(),
            flag::FORMAT.into(),
            "json".into(),
        ],
    )
    .await?;
    let documents = result.stdout_json::<ComponentGetView>();
    ensure!(documents.len() == 1, "expected one deployed component");
    let component = &documents[0].0;
    ensure!(
        component.component_size == bytes.len() as u64,
        "deployed component size differs from componentWasm"
    );
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?
        .get(format!(
            "{}/v1/components/{}/revisions/{}/wasm",
            ctx.worker_service_url(),
            component.component_id,
            component.component_revision
        ))
        .bearer_auth(golem_client::LOCAL_WELL_KNOWN_TOKEN)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    ensure!(
        response.as_ref() == bytes,
        "downloaded deployment differs from selected componentWasm"
    );
    Ok((path, bytes))
}

fn canonical(value: &TypedSchemaValue) -> anyhow::Result<Value> {
    to_json_value(value.graph(), value.root_type(), value.value()).map_err(Into::into)
}

fn checked_value(value: &TypedSchemaValue, expected: &Value) -> anyhow::Result<Value> {
    let expected_value = from_json_value(value.graph(), value.root_type(), expected)?;
    ensure!(
        value.value() == &expected_value,
        "expected {expected}, got {}",
        canonical(value)?
    );
    canonical(value)
}

async fn checked_call(
    ctx: &TestContext,
    descriptor: &FixtureDescriptor,
    call: &CheckedCall,
) -> anyhow::Result<(Value, Option<OwnedInvocationId>)> {
    match &call.target {
        CallTarget::Invoke { agent, method } => {
            let binding = descriptor
                .agents
                .get(agent)
                .context("missing agent binding")?;
            let mut args = vec![
                cmd::AGENT.into(),
                cmd::INVOKE.into(),
                "--no-stream".into(),
                binding.agent_id(),
                method.clone(),
            ];
            args.extend(
                call.arguments
                    .as_array()
                    .context("invocation arguments must be an array")?
                    .iter()
                    .map(Value::to_string),
            );
            args.extend([flag::FORMAT.into(), "json".into()]);
            let output = command(ctx, args).await?;
            let mut values = output
                .stdout()
                .map(serde_json::from_str::<InvokeResultView>);
            let result = values.next().context("missing invocation result")??;
            ensure!(values.next().is_none(), "multiple invocation results");
            let invocation_id = result.idempotency_key.parse()?;
            let result = result
                .result_json
                .as_ref()
                .context("missing typed invocation result")?;
            let metadata = command(
                ctx,
                vec![
                    cmd::AGENT.into(),
                    "get".into(),
                    binding.agent_id(),
                    flag::FORMAT.into(),
                    "json".into(),
                ],
            )
            .await?;
            let metadata = metadata.stdout_json::<AgentGetView>();
            ensure!(metadata.len() == 1, "expected one invoked agent");
            Ok((
                checked_value(result, &call.expected)?,
                Some(OwnedInvocationId::new(
                    metadata[0].metadata.agent_id,
                    invocation_id,
                )),
            ))
        }
        CallTarget::Http { method, path } => {
            let expected: HttpExpected = serde_json::from_value(call.expected.clone())?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()?;
            let mut request = client
                .request(
                    method.parse()?,
                    format!("http://localhost:{}{path}", ctx.custom_request_port()),
                )
                .header(
                    reqwest::header::HOST,
                    format!(
                        "{}.localhost:{}",
                        ctx.cwd_path()
                            .file_name()
                            .and_then(|name| name.to_str())
                            .context("fixture app name missing")?,
                        ctx.custom_request_port()
                    ),
                );
            if !call.arguments.is_null() {
                request = request.json(&call.arguments);
            }
            let response = request.send().await?;
            let status = response.status().as_u16();
            let actual: Value = response.json().await?;
            ensure!(
                status == expected.status,
                "expected HTTP {}, got {status}",
                expected.status
            );
            ensure!(
                actual == expected.body,
                "expected HTTP body {}, got {actual}",
                expected.body
            );
            Ok((actual, None))
        }
        CallTarget::Tool { component, tool } => {
            let arguments: ToolArguments = serde_json::from_value(call.arguments.clone())?;
            let expected: ToolExpected = serde_json::from_value(call.expected.clone())?;
            let discovery = command(
                ctx,
                vec![
                    "tool".into(),
                    "get".into(),
                    tool.clone(),
                    flag::FORMAT.into(),
                    "json".into(),
                ],
            )
            .await?;
            let tools = discovery.stdout_json::<DeployedToolView>();
            ensure!(tools.len() == 1, "expected one discovered tool");
            let definition = &tools[0].tool.definition;
            ensure!(
                definition.name() == Some(tool.as_str()),
                "discovered tool name differs from descriptor"
            );
            let golem_common::schema::tool::argv::ParsedToolArguments::Invoke {
                command_path, ..
            } = golem_common::schema::tool::argv::parse(definition, &arguments.argv)
                .map_err(anyhow::Error::msg)?
            else {
                bail!("fixture requested tool help rather than an invocation");
            };
            let index = definition
                .command_index_by_path(&command_path)
                .context("discovered tool has no requested command")?;
            let body = definition.commands.nodes[index]
                .body
                .as_ref()
                .context("tool command has no body")?;
            ensure!(
                body.stdin.is_some() || arguments.stdin.is_empty(),
                "descriptor supplies stdin to a command without stdin"
            );
            ensure!(
                body.stdout.is_some() || expected.stdout.is_empty(),
                "descriptor expects stdout from a command without stdout"
            );
            ensure!(
                body.stderr.is_some() || expected.stderr.is_empty(),
                "descriptor expects stderr from a command without stderr"
            );
            let directory =
                ctx.cwd_path_join(format!("golem-temp/size-evidence/{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory)?;
            let input = directory.join("stdin");
            let output = directory.join("stdout");
            let error_output = directory.join("stderr");
            std::fs::write(&input, &arguments.stdin)?;
            let mut args = vec![
                "tool".into(),
                "invoke".into(),
                "--component".into(),
                component.clone(),
                tool.clone(),
                flag::FORMAT.into(),
                "json".into(),
            ];
            let streaming = body.stdin.is_some() || body.stdout.is_some() || body.stderr.is_some();
            if body.stdin.is_some() {
                args.extend(["--stdin".into(), input.display().to_string()]);
            }
            if body.stdout.is_some() {
                args.extend([
                    "--stdout".into(),
                    "--output".into(),
                    output.display().to_string(),
                ]);
            }
            if body.stderr.is_some() {
                args.extend([
                    "--stderr".into(),
                    "--stderr-output".into(),
                    error_output.display().to_string(),
                ]);
            }
            args.push("--".into());
            args.extend(arguments.argv);
            let result = command(ctx, args).await?;
            let mut documents = result.stdout().map(serde_json::from_str::<Value>);
            let document = documents.next().context("missing tool result")??;
            ensure!(documents.next().is_none(), "multiple tool results");
            let owner = if streaming {
                document.get("invocationId")
            } else {
                document.pointer("/response/invocationId")
            }
            .and_then(Value::as_str)
            .context("tool result did not identify its invocation")?
            .parse::<OwnedInvocationId>()
            .map_err(anyhow::Error::msg)?;
            ensure!(
                matches!(owner.owner(), InvocationOwner::Tool(name) if name.as_str() == tool.as_str()),
                "tool invocation belongs to a different owner"
            );
            let actual = if streaming {
                match serde_json::from_value::<PublicInvocationResult>(
                    document
                        .get("result")
                        .context("missing tool session result")?
                        .clone(),
                )? {
                    PublicInvocationResult::ToolSuccess {
                        result: Some(value),
                    } => {
                        let value =
                            TypedSchemaValue::try_from(value).map_err(anyhow::Error::msg)?;
                        canonical(&value)?
                    }
                    PublicInvocationResult::ToolSuccess { result: None } => Value::Null,
                    result => bail!("tool session did not succeed: {result:?}"),
                }
            } else {
                let response: NativeToolInvocationResponse = serde_json::from_value(
                    document
                        .get("response")
                        .context("missing tool response")?
                        .clone(),
                )?;
                match response.result.context("missing final tool result")? {
                    NativeToolResult::Success(success) => match success.result {
                        Some(value) => canonical(value.as_inner())?,
                        None => Value::Null,
                    },
                    result => bail!("tool did not succeed: {result:?}"),
                }
            };
            ensure!(
                actual == expected.result,
                "expected tool result {}, got {actual}",
                expected.result
            );
            if body.stdout.is_some() {
                ensure!(
                    std::fs::read(output)? == expected.stdout,
                    "tool stdout differs"
                );
            }
            if body.stderr.is_some() {
                ensure!(
                    std::fs::read(error_output)? == expected.stderr,
                    "tool stderr differs"
                );
            }
            Ok((actual, Some(owner)))
        }
    }
}

async fn no_network(
    ctx: &TestContext,
    component: &str,
    invocation_id: &OwnedInvocationId,
) -> anyhow::Result<()> {
    let output = command(
        ctx,
        vec![
            "component".into(),
            "get".into(),
            component.into(),
            flag::FORMAT.into(),
            "json".into(),
        ],
    )
    .await?;
    let components = output.stdout_json::<ComponentGetView>();
    ensure!(components.len() == 1, "expected one deployed component");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let owner = match invocation_id.owner() {
        InvocationOwner::Agent(agent_id) => *agent_id,
        InvocationOwner::Tool(_) => {
            let invocation: golem_client::model::Invocation = client
                .get(format!(
                    "{}/v1/runtime/envs/{}/invocations/{invocation_id}",
                    ctx.worker_service_url(),
                    components[0].0.environment_id,
                ))
                .bearer_auth(golem_client::LOCAL_WELL_KNOWN_TOKEN)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            ensure!(
                invocation.summary.invocation_id == invocation_id.to_string(),
                "invocation lookup returned a different invocation"
            );
            invocation.summary.agent_id.into()
        }
    };
    let mut entries: Vec<AgentOplogEntryView> = Vec::new();
    let mut from = 1;
    loop {
        let mut url = url::Url::parse(&format!(
            "{}/v1/runtime/envs/{}/agents/unused/oplog",
            ctx.worker_service_url(),
            components[0].0.environment_id
        ))?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid oplog URL"))?
            .pop()
            .pop()
            .push(&owner.to_string())
            .push("oplog");
        let page: golem_client::model::GetOplogResponse = client
            .get(url)
            .query(&[("from", from), ("count", 1000)])
            .bearer_auth(golem_client::LOCAL_WELL_KNOWN_TOKEN)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(
            !page.entries.is_empty(),
            "oplog ended before its last index"
        );
        for entry in page.entries {
            entries.push(match entry {
                golem_common::model::oplog::PublicOplogPageEntry::Included(entry) => entry.into(),
                golem_common::model::oplog::PublicOplogPageEntry::Omitted(entry) => entry.into(),
            });
        }
        let last = entries.last().context("empty oplog page")?.index;
        ensure!(last >= from, "oplog pagination did not advance");
        if last == page.last_index {
            break;
        }
        ensure!(last < page.last_index, "oplog exceeds its last index");
        from = last + 1;
    }
    check_no_network(&entries)?;
    if let InvocationOwner::Tool(tool_name) = invocation_id.owner() {
        let AgentOplogEntryContent::Full(PublicOplogEntry::Create(create)) = &entries[0].entry
        else {
            bail!("tool invocation oplog does not begin with Create");
        };
        let tool = create
            .standalone_tool
            .as_ref()
            .context("tool invocation oplog has no standalone tool identity")?;
        ensure!(
            &tool.tool_name == tool_name && tool.invocation_id == invocation_id.id(),
            "oplog belongs to a different tool invocation"
        );
    }
    Ok(())
}

fn check_no_network(entries: &[AgentOplogEntryView]) -> anyhow::Result<()> {
    ensure!(!entries.is_empty(), "first-call oplog is empty");
    ensure!(
        entries[0].index == 1
            && matches!(
                entries[0].entry,
                AgentOplogEntryContent::Full(PublicOplogEntry::Create(_))
            ),
        "first-call oplog does not begin with Create"
    );
    let mut started = std::collections::HashSet::new();
    let mut finished = 0;
    let mut previous = None;
    for view in entries {
        if let Some(index) = previous {
            ensure!(view.index == index + 1, "first-call oplog has a gap");
        }
        previous = Some(view.index);
        let AgentOplogEntryContent::Full(entry) = &view.entry else {
            bail!("first-call oplog omitted an entry");
        };
        match entry {
            PublicOplogEntry::Create(_) => {
                ensure!(view.index == 1, "first-call oplog repeats Create")
            }
            PublicOplogEntry::AgentInvocationStarted(call) => {
                let id = match &call.invocation {
                    PublicAgentInvocation::AgentInitialization(call) => call.invocation_id,
                    PublicAgentInvocation::AgentMethodInvocation(call) => call.invocation_id,
                    PublicAgentInvocation::ExternalTool(call) => call.invocation_id,
                    PublicAgentInvocation::ProcessOplogEntries(call) => call.invocation_id,
                    invocation => bail!("unexpected first-call invocation: {invocation:?}"),
                };
                ensure!(started.insert(id), "first-call invocation started twice");
            }
            PublicOplogEntry::AgentInvocationFinished(call) => {
                ensure!(
                    started.remove(&call.invocation_id),
                    "first-call completion has no matching invocation"
                );
                finished += 1;
            }
            PublicOplogEntry::AgentInvocationFailed(_) => {
                bail!("first-call oplog contains a failed invocation")
            }
            PublicOplogEntry::Start(call) => ensure!(
                !call.function_name.starts_with("sockets::")
                    && !call.function_name.starts_with("wasi:sockets/")
                    && !call.function_name.starts_with("golem::network::")
                    && !call.function_name.starts_with("http::")
                    && !call.function_name.starts_with("wasi:http/")
                    && !call.function_name.starts_with("rdbms::")
                    && !call.function_name.starts_with("golem:websocket/"),
                "first call attempted outbound network: {}",
                call.function_name
            ),
            _ => {}
        }
    }
    ensure!(
        finished > 0 && started.is_empty(),
        "first-call oplog is incomplete"
    );
    Ok(())
}

async fn tool_output(
    ctx: &TestContext,
    dir: &Path,
    program: &str,
    args: &[&str],
) -> anyhow::Result<Vec<u8>> {
    let output = tokio::time::timeout(
        Duration::from_secs(1200),
        tokio::process::Command::new(program)
            .args(args)
            .envs(&ctx.env)
            .current_dir(dir)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("fixture tool exceeded its deadline")??;
    ensure!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

async fn collect_fixture(
    ctx: &TestContext,
    fixture: Fixture,
    preset: &str,
    component: &str,
    path: &Path,
    uploaded: &[u8],
) -> anyhow::Result<(Vec<u8>, u64)> {
    let directory = component_directory(ctx, component)?;
    let snake = component.to_snake_case();
    let evidence = ctx.cwd_path_join("golem-temp/size-evidence");
    std::fs::create_dir_all(&evidence)?;
    let mut inputs = CollectInputs {
        uploaded,
        ..Default::default()
    };
    let (guest, module, named);
    let user;
    match fixture.sdk {
        Sdk::Ts | Sdk::Effect | Sdk::Scala => {
            let guest_path = match fixture.sdk {
                Sdk::Ts => {
                    ctx.cwd_path_join("node_modules/@golemcloud/golem-ts-sdk/wasm/agent_guest.wasm")
                }
                Sdk::Effect => {
                    ctx.cwd_path_join("node_modules/@golemcloud/effect-golem/wasm/agent_guest.wasm")
                }
                _ => ctx.cwd_path_join(".generated/agent_guest.wasm"),
            };
            guest = std::fs::read(guest_path)?;
            let module_path = if fixture.sdk == Sdk::Scala {
                ctx.cwd_path_join(format!(".golem/{snake}.js"))
            } else {
                ctx.cwd_path_join(format!(
                    "golem-temp/ts-dist/{}/main.js",
                    component.to_kebab_case()
                ))
            };
            module = std::fs::read_to_string(module_path)?;
            inputs.guest = Some(&guest);
            inputs.module = Some(&module);
            if fixture.sdk != Sdk::Scala {
                let bundle_dir =
                    ctx.cwd_path_join(format!("golem-temp/ts-dist/{}", component.to_kebab_case()));
                let map = read_module_map(&bundle_dir.join("main.modules.json"))?;
                ensure!(
                    map.keys().all(|id| !Path::new(id).is_absolute()
                        && !id.contains(&ctx.cwd_path().display().to_string())),
                    "the module map contains an absolute application path"
                );
                if fixture.kind == FixtureKind::RepresentativeAgent {
                    ensure!(
                        map.iter().any(
                            |(id, bytes)| id.ends_with("counter-agent.ts") && *bytes >= 100_000
                        ),
                        "final bundle map does not retain the user literal"
                    );
                    ensure!(
                        map.iter()
                            .any(|(id, bytes)| id.contains("base64-js") && *bytes > 0),
                        "final bundle map has no used base64-js dependency"
                    );
                }
                inputs.module_map = Some(map);
            } else if fixture.kind == FixtureKind::RepresentativeAgent {
                let bundle = std::fs::read(ctx.cwd_path_join(format!(".golem/{snake}.js")))?;
                let payload = std::fs::read(
                    crate::workspace_path()
                        .join("test-components/size-analysis/sdk-fixtures/payload.txt"),
                )?;
                ensure!(
                    payload
                        .chunks(50_000)
                        .all(|chunk| bundle.windows(chunk.len()).any(|window| window == chunk)),
                    "linked Scala bundle does not retain the literal"
                );
            }
        }
        Sdk::Rust => {
            user = snake.clone();
            inputs.user = Some(&user);
            if preset == "release" {
                let target = ctx.data_dir.path().join("size-named-target");
                tool_output(
                    ctx,
                    &directory,
                    "cargo",
                    &[
                        "build",
                        "--target",
                        "wasm32-wasip2",
                        "--release",
                        "--config",
                        "profile.release.strip=\"none\"",
                        "--target-dir",
                        target.to_str().context("non-UTF8 named target")?,
                    ],
                )
                .await?;
                named = std::fs::read(target.join(format!("wasm32-wasip2/release/{snake}.wasm")))?;
                inputs.named = Some(&named);
            }
        }
        Sdk::Go => {
            let manifest = std::fs::read_to_string(directory.join("go.mod"))?;
            user = manifest
                .lines()
                .find_map(|line| line.strip_prefix("module "))
                .context("missing Go module")?
                .trim()
                .to_owned();
            inputs.user = Some(&user);
            named = if preset == "release" {
                std::fs::read(path.with_extension("debug.wasm"))?
            } else {
                uploaded.to_vec()
            };
            inputs.named = Some(&named);
            let packages = tokio::time::timeout(
                Duration::from_secs(1200),
                tokio::process::Command::new("go")
                    .args([
                        "list",
                        "-deps",
                        "-json",
                        "-buildmode=c-shared",
                        "-tags=componentizego_async",
                        ".",
                    ])
                    .envs(&ctx.env)
                    .env("GOOS", "wasip1")
                    .env("GOARCH", "wasm")
                    .current_dir(&directory)
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .context("Go package resolution exceeded its deadline")??;
            ensure!(
                packages.status.success(),
                "Go package resolution failed: {}",
                String::from_utf8_lossy(&packages.stderr)
            );
            std::fs::write(evidence.join("go-packages.json"), &packages.stdout)?;
            inputs.go_packages = Some(read_go_packages(&packages.stdout)?);
        }
        Sdk::Moonbit => {
            let manifest = std::fs::read_to_string(ctx.cwd_path_join("moon.mod"))?;
            user = manifest
                .lines()
                .find_map(|line| line.strip_prefix("name = "))
                .context("missing MoonBit module")?
                .trim_matches('"')
                .to_owned();
            inputs.user = Some(&user);
            named = if preset == "release" {
                command(
                    ctx,
                    vec![
                        flag::YES.into(),
                        "-P".into(),
                        "debug".into(),
                        "build".into(),
                    ],
                )
                .await?;
                std::fs::read(selected_wasm(ctx, "debug", component).await?)?
            } else {
                uploaded.to_vec()
            };
            inputs.named = Some(&named);
        }
    }
    std::fs::write(evidence.join(format!("{snake}-{preset}.wasm")), uploaded)?;
    if let Some(named) = inputs.named {
        std::fs::write(evidence.join(format!("{snake}-{preset}-named.wasm")), named)?;
    }
    let (measurement, diagnostics) = collect(fixture.sdk, &inputs)?;
    ensure!(
        measurement
            .sections
            .iter()
            .map(|row| row.bytes)
            .sum::<u64>()
            == uploaded.len() as u64,
        "section accounting differs from upload"
    );
    ensure!(
        measurement
            .breakdown
            .buckets
            .iter()
            .map(|row| row.bytes)
            .sum::<u64>()
            == uploaded.len() as u64,
        "bucket accounting differs from upload"
    );
    std::fs::write(
        evidence.join(format!("{snake}-{preset}.json")),
        to_json(&measurement)?,
    )?;
    std::fs::write(
        evidence.join(format!("{snake}-{preset}-diagnostics.json")),
        to_json(&diagnostics)?,
    )?;
    if fixture.kind == FixtureKind::RepresentativeAgent {
        let dependency = match fixture.sdk {
            Sdk::Rust => Some("hex"),
            Sdk::Go => Some("github.com/dustin/go-humanize"),
            Sdk::Moonbit => Some("moonbitlang/x"),
            _ => None,
        };
        if let Some(dependency) = dependency {
            let namespaces: Vec<_> = diagnostics
                .namespaces
                .iter()
                .filter(|(name, _)| name.contains(dependency))
                .collect();
            let symbols: Vec<_> = diagnostics
                .symbols
                .iter()
                .filter(|symbol| {
                    fixture.sdk == Sdk::Go
                        && golem_size_analysis::breakdown::go_symbol_matches(
                            &symbol.name,
                            dependency,
                        )
                })
                .collect();
            std::fs::write(
                evidence.join("dependency-attribution.json"),
                to_json(&serde_json::json!({
                    "dependency": dependency,
                    "namespaces": namespaces,
                    "symbols": symbols,
                    "approximate": measurement.breakdown.approximate,
                    "dependencyBytes": measurement.breakdown.bucket(Bucket::Dependencies),
                }))?,
            )?;
            if matches!(fixture.sdk, Sdk::Go | Sdk::Moonbit) {
                ensure!(
                    (!namespaces.is_empty() || !symbols.is_empty())
                        && measurement.breakdown.bucket(Bucket::Dependencies) > 0,
                    "named companion has no attribution for {dependency}; inspect diagnostics"
                );
            }
        }
    }
    let runtime = measurement.breakdown.bucket(Bucket::Runtime);
    Ok((inputs.guest.unwrap_or_default().to_vec(), runtime))
}

async fn source_resolution(
    ctx: &TestContext,
    fixture: Fixture,
    component: &str,
) -> anyhow::Result<()> {
    let root = crate::workspace_path().canonicalize()?;
    let directory = component_directory(ctx, component)?;
    let sdk = fixture.sdk;
    let representative = fixture.kind == FixtureKind::RepresentativeAgent;
    let evidence = match sdk {
        Sdk::Ts | Sdk::Effect => {
            let (package, source) = if sdk == Sdk::Ts {
                ("@golemcloud/golem-ts-sdk", "sdks/ts/packages/golem-ts-sdk")
            } else {
                ("@golemcloud/effect-golem", "sdks/effect")
            };
            let actual = ctx
                .cwd_path_join(format!("node_modules/{package}"))
                .canonicalize()?;
            ensure!(
                actual == root.join(source).canonicalize()?,
                "Node SDK did not resolve to this checkout: {}",
                actual.display()
            );
            let lock: Value =
                serde_json::from_slice(&std::fs::read(ctx.cwd_path_join("package-lock.json"))?)?;
            ensure!(
                lock["packages"][format!("node_modules/{package}")]["link"] == true,
                "Node lock did not resolve a local SDK link"
            );
            if representative {
                ensure!(
                    lock["packages"]["node_modules/base64-js"]["version"] == "1.5.1",
                    "Node did not resolve the pinned representative dependency"
                );
            }
            serde_json::to_vec_pretty(&lock)?
        }
        Sdk::Rust => {
            let metadata = tool_output(
                ctx,
                &directory,
                "cargo",
                &["metadata", "--format-version", "1", "--locked"],
            )
            .await?;
            let value: Value = serde_json::from_slice(&metadata)?;
            let sdk = value["packages"]
                .as_array()
                .context("Cargo packages missing")?
                .iter()
                .find(|package| package["name"] == "golem-rust")
                .context("Cargo did not resolve golem-rust")?;
            let path = Path::new(
                sdk["manifest_path"]
                    .as_str()
                    .context("Cargo SDK manifest missing")?,
            )
            .canonicalize()?;
            ensure!(
                path == root
                    .join("sdks/rust/golem-rust/Cargo.toml")
                    .canonicalize()?,
                "Cargo resolved another SDK"
            );
            let id = &sdk["id"];
            ensure!(
                value["resolve"]["nodes"]
                    .as_array()
                    .context("Cargo resolve graph missing")?
                    .iter()
                    .any(|node| &node["id"] == id),
                "local Rust SDK is not in the resolved graph"
            );
            if representative {
                ensure!(
                    value["packages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|package| package["name"] == "hex" && package["version"] == "0.4.3"),
                    "Cargo did not resolve the pinned representative dependency"
                );
            }
            metadata
        }
        Sdk::Go => {
            let modules =
                tool_output(ctx, &directory, "go", &["list", "-m", "-json", "all"]).await?;
            let values = serde_json::Deserializer::from_slice(&modules)
                .into_iter::<Value>()
                .collect::<Result<Vec<_>, _>>()?;
            for module in ["golem", "core"] {
                let name = format!("github.com/golemcloud/golem/sdks/go/{module}");
                let value = values
                    .iter()
                    .find(|value| value["Path"] == name)
                    .context("Go SDK module missing")?;
                let path = Path::new(
                    value["Replace"]["Dir"]
                        .as_str()
                        .context("Go SDK replacement missing")?,
                )
                .canonicalize()?;
                ensure!(
                    path == root.join(format!("sdks/go/{module}")).canonicalize()?,
                    "Go resolved another SDK module"
                );
            }
            if representative {
                ensure!(
                    values
                        .iter()
                        .any(|module| module["Path"] == "github.com/dustin/go-humanize"
                            && module["Version"] == "v1.0.1"),
                    "Go did not resolve the pinned representative dependency"
                );
            }
            modules
        }
        Sdk::Moonbit => {
            let tree = tool_output(ctx, ctx.cwd_path(), "moon", &["tree", "--json"]).await?;
            let value: Value = serde_json::from_slice(&tree)?;
            ensure!(
                value["status"] == "success",
                "MoonBit module resolution failed"
            );
            let sdk = value["modules"]
                .as_array()
                .context("MoonBit modules missing")?
                .iter()
                .find(|module| module["name"] == "golemcloud/golem_sdk")
                .context("MoonBit SDK missing")?;
            ensure!(sdk["source"]["kind"] == "local", "MoonBit SDK is not local");
            let path = Path::new(
                sdk["source"]["path"]
                    .as_str()
                    .context("MoonBit SDK path missing")?,
            )
            .canonicalize()?;
            ensure!(
                path == root.join("sdks/moonbit/golem_sdk").canonicalize()?,
                "MoonBit resolved another SDK"
            );
            if representative {
                ensure!(
                    value["modules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|module| module["name"] == "moonbitlang/x"
                            && module["version"] == "0.5.1"),
                    "MoonBit did not resolve the pinned representative dependency"
                );
            }
            tree
        }
        Sdk::Scala => {
            let command = format!(
                "show {}/Compile/dependencyClasspath",
                component.to_snake_case()
            );
            let classpath = tool_output(
                ctx,
                ctx.cwd_path(),
                "sbt",
                &["-batch", "-no-colors", "-Dsbt.supershell=false", &command],
            )
            .await?;
            let text = String::from_utf8(classpath.clone())?;
            if representative {
                ensure!(
                    text.contains("/ujson_sjs1_3-3.1.0.jar)"),
                    "Scala did not resolve the pinned representative dependency"
                );
            }
            let mut resolved = std::collections::BTreeSet::new();
            for line in text.lines() {
                let Some((_, path)) = line.split_once("Attributed(") else {
                    continue;
                };
                let path = Path::new(path.trim_end_matches(')'));
                let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !file.starts_with("golem-scala-") || !file.ends_with(".jar") {
                    continue;
                }
                ensure!(
                    path.to_string_lossy().contains("/local/cloud.golem/")
                        && path.to_string_lossy().contains("/0.0.0-SNAPSHOT/"),
                    "Scala resolved a nonlocal SDK: {}",
                    path.display()
                );
                let stem = file
                    .trim_end_matches(".jar")
                    .trim_end_matches("-0.0.0-SNAPSHOT");
                let built_name = format!("{stem}-0.0.0-SNAPSHOT.jar");
                let built = walkdir::WalkDir::new(root.join("sdks/scala"))
                    .into_iter()
                    .filter_map(Result::ok)
                    .find(|entry| {
                        entry.file_type().is_file() && entry.file_name() == built_name.as_str()
                    })
                    .context("resolved Scala jar has no build output in this checkout")?;
                ensure!(
                    std::fs::read(path)? == std::fs::read(built.path())?,
                    "resolved Scala SDK differs from this checkout's published jar"
                );
                resolved.insert(stem.to_owned());
            }
            ensure!(
                resolved.iter().any(|name| name.contains("-core_"))
                    && resolved.iter().any(|name| name.contains("-model_"))
                    && resolved.iter().any(|name| name.contains("-macros_")),
                "Scala classpath omitted required local SDK jars"
            );
            classpath
        }
    };
    let path = ctx.cwd_path_join("golem-temp/size-evidence/source-resolution.json");
    std::fs::create_dir_all(path.parent().context("evidence parent missing")?)?;
    std::fs::write(path, evidence)?;
    Ok(())
}

async fn unused_import(
    ctx: &TestContext,
    sdk: Sdk,
    preset: &str,
    component: &str,
) -> anyhow::Result<()> {
    if !matches!(sdk, Sdk::Rust | Sdk::Go | Sdk::Moonbit) {
        return Ok(());
    }
    let wasm = selected_wasm(ctx, preset, component).await?;
    let baseline = std::fs::read(&wasm)?;
    let directory = component_directory(ctx, component)?;
    let (path, original, variant) = match sdk {
        Sdk::Rust => {
            let path = directory.join("src/lib.rs");
            let original = std::fs::read_to_string(&path)?;
            let variant = format!("{original}\nuse std::net::TcpStream as UnusedSizeFixtureSocket;\n");
            (path, Some(original), variant)
        }
        Sdk::Moonbit => {
            let path = directory.join("moon.pkg");
            let original = std::fs::read_to_string(&path)?;
            let variant = original.replacen("import {", "import {\n  \"golemcloud/golem_sdk/http\" @unused_size_fixture,", 1);
            ensure!(original != variant, "MoonBit package has no import block");
            (path, Some(original), variant)
        }
        Sdk::Go => (directory.join("unused_size_fixture.go"), None, "package main\nimport \"encoding/hex\"\nfunc unusedSizeFixture() { _ = hex.EncodeToString }\n".into()),
        _ => unreachable!(),
    };
    std::fs::write(&path, variant)?;
    let result: anyhow::Result<()> = async {
        command(
            ctx,
            vec![
                flag::YES.into(),
                "-P".into(),
                preset.into(),
                "build".into(),
                "--force-build".into(),
            ],
        )
        .await?;
        let actual = std::fs::read(&wasm)?;
        let evidence = ctx.cwd_path_join("golem-temp/size-evidence");
        std::fs::create_dir_all(&evidence)?;
        std::fs::write(evidence.join("unused-import.wasm"), &actual)?;
        ensure!(
            actual == baseline,
            "Rule 3: an unused feature import changed uploaded bytes ({} → {})",
            baseline.len(),
            actual.len()
        );
        Ok(())
    }
    .await;
    match original {
        Some(source) => std::fs::write(path, source)?,
        None => std::fs::remove_file(path)?,
    }
    std::fs::write(wasm, baseline)?;
    result
}

fn copy_inputs(source: &Path, target: &Path) -> anyhow::Result<()> {
    for entry in walkdir::WalkDir::new(source)
        .into_iter()
        .filter_entry(|entry| {
            ![
                "target",
                "node_modules",
                "golem-temp",
                ".agents",
                ".generated",
                ".golem",
                "_build",
                "dist",
            ]
            .contains(&entry.file_name().to_str().unwrap_or(""))
        })
    {
        let entry = entry?;
        if entry.file_type().is_file() {
            golem_cli::fs::copy(
                entry.path(),
                target.join(entry.path().strip_prefix(source)?),
            )?;
        }
    }
    Ok(())
}

fn component_directory(ctx: &TestContext, component: &str) -> anyhow::Result<PathBuf> {
    let manifest: serde_yaml::Value =
        serde_yaml::from_slice(&std::fs::read(ctx.cwd_path_join("golem.yaml"))?)?;
    let directory = manifest["components"][component]["dir"]
        .as_str()
        .context("generated component has no directory")?;
    Ok(ctx.cwd_path_join(directory))
}

fn prepare_tool(ctx: &TestContext, sdk: Sdk) -> anyhow::Result<()> {
    let root = crate::workspace_path();
    let component = component_directory(ctx, "size-fixture:provider")?;
    match sdk {
        Sdk::Rust => {
            std::fs::remove_dir_all(component.join("src"))?;
            let source = root.join("test-components/tool-streaming/rust-provider");
            copy_inputs(&source.join("src"), &component.join("src"))?;
            let manifest = std::fs::read_to_string(source.join("Cargo.toml"))?;
            std::fs::write(
                component.join("Cargo.toml"),
                manifest
                    .replace(
                        "../../../sdks/rust/golem-rust",
                        &root.join("sdks/rust/golem-rust").display().to_string(),
                    )
                    .replace(
                        "golem_it_tool_streaming_rust_provider",
                        "size_fixture_provider",
                    ),
            )?;
            golem_cli::fs::copy(source.join("Cargo.lock"), component.join("Cargo.lock"))?;
            let bindings = component.join("src/wallet_recovery.rs");
            let contents = std::fs::read_to_string(&bindings)?;
            std::fs::write(
                bindings,
                contents.replace(
                    "../../../sdks/rust/golem-rust/wit",
                    &root.join("sdks/rust/golem-rust/wit").display().to_string(),
                ),
            )?;
        }
        Sdk::Ts | Sdk::Effect => {
            let application = if sdk == Sdk::Ts {
                "tool-streaming-ts"
            } else {
                "tool-streaming-effect"
            };
            std::fs::remove_dir_all(component.join("src"))?;
            copy_inputs(
                &root
                    .join("test-components")
                    .join(application)
                    .join("provider/src"),
                &component.join("src"),
            )?;
        }
        Sdk::Go => {
            std::fs::remove_dir_all(component.join("agents"))?;
            copy_inputs(
                &root.join("test-components/tool-streaming-go/provider"),
                &component,
            )?;
        }
        Sdk::Moonbit => {
            // Keep the combined provider/caller package intact; registration is regenerated.
            for entry in std::fs::read_dir(&component)? {
                let path = entry?.path();
                if path.extension().is_some_and(|extension| extension == "mbt") {
                    std::fs::remove_file(path)?;
                }
            }
            copy_inputs(
                &root.join("test-components/tool-streaming-moonbit/component"),
                &component,
            )?;
        }
        Sdk::Scala => {
            std::fs::remove_dir_all(component.join("src"))?;
            copy_inputs(
                &root.join("test-components/tool-streaming-scala/src"),
                &component.join("src"),
            )?;
            let build = ctx.cwd_path_join("size_fixture_provider.sbt");
            let contents = std::fs::read_to_string(&build)?;
            std::fs::write(
                build,
                contents.replace("Some(\"size_fixture_provider\")", "Some(\"toolstreaming\")"),
            )?;
        }
    }
    let manifest = ctx.cwd_path_join("golem.yaml");
    let mut yaml: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(&manifest)?)?;
    yaml.as_mapping_mut()
        .context("manifest is not an object")?
        .remove("httpApi");
    let (application, provider) = match sdk {
        Sdk::Rust => ("tool-streaming", "golem-it:tool-streaming-rust-provider"),
        Sdk::Ts => ("tool-streaming-ts", "golem-it:tool-streaming-ts-provider"),
        Sdk::Effect => (
            "tool-streaming-effect",
            "golem-it:tool-streaming-effect-provider",
        ),
        Sdk::Go => ("tool-streaming-go", "golem-it:tool-streaming-go-provider"),
        Sdk::Moonbit => ("tool-streaming-moonbit", "golem:moonbit-examples"),
        Sdk::Scala => ("tool-streaming-scala", "scala:examples"),
    };
    let source: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(
        root.join("test-components")
            .join(application)
            .join("golem.yaml"),
    )?)?;
    // Discovery requires declarations for every exported tool, even the ones S1 never calls.
    for (name, declaration) in source["tools"]
        .as_mapping()
        .context("provider tools missing")?
    {
        if name.as_str() == Some("middleware") {
            for (middleware, declaration) in declaration
                .as_mapping()
                .context("middleware declarations missing")?
            {
                if declaration["component"].as_str() == Some(provider) {
                    let mut declaration = declaration.clone();
                    declaration["component"] = "size-fixture:provider".into();
                    yaml["tools"]["middleware"][middleware.clone()] = declaration;
                }
            }
            continue;
        }
        if declaration
            .get("component")
            .and_then(serde_yaml::Value::as_str)
            .is_some_and(|component| component != provider)
        {
            continue;
        }
        let mut declaration = declaration.clone();
        declaration["component"] = "size-fixture:provider".into();
        yaml["tools"][name.clone()] = declaration;
    }
    if let Some(resources) = source.get("resourceDefaults") {
        yaml["resourceDefaults"] = resources.clone();
    }
    std::fs::write(manifest, serde_yaml::to_string(&yaml)?)?;
    Ok(())
}

async fn prepare_representative(ctx: &TestContext, sdk: Sdk, preset: &str) -> anyhow::Result<()> {
    command(
        ctx,
        vec![
            flag::YES.into(),
            cmd::NEW.into(),
            ".".into(),
            flag::TEMPLATE.into(),
            sdk.name().into(),
            flag::COMPONENT_NAME.into(),
            "size-fixture:provider".into(),
        ],
    )
    .await?;
    prepare_tool(ctx, sdk)?;
    if sdk == Sdk::Rust {
        let main = component_directory(ctx, "size-fixture:main")?.join("Cargo.toml");
        let mut package: toml::Table = std::fs::read_to_string(&main)?.parse()?;
        let mut workspace = toml::Table::new();
        workspace.insert(
            "workspace".into(),
            toml::toml! {
                members = ["size-fixture-main", "size-fixture-provider"]
                resolver = "3"
            }
            .into(),
        );
        for key in ["profile", "patch"] {
            if let Some(value) = package.remove(key) {
                workspace.insert(key.into(), value);
            }
        }
        std::fs::write(main, toml::to_string(&package)?)?;
        std::fs::write(
            ctx.cwd_path_join("Cargo.toml"),
            toml::to_string(&workspace)?,
        )?;
    }
    if matches!(sdk, Sdk::Moonbit | Sdk::Rust) {
        command(
            ctx,
            vec![
                flag::YES.into(),
                "-P".into(),
                preset.into(),
                cmd::BUILD.into(),
                "size-fixture:provider".into(),
            ],
        )
        .await?;
    }
    let root = crate::workspace_path();
    let main = component_directory(ctx, "size-fixture:main")?;
    let overlay = root
        .join("test-components/size-analysis/sdk-fixtures")
        .join(sdk.name())
        .join("representative");
    ensure!(
        overlay.is_dir(),
        "missing {} representative source",
        sdk.name()
    );
    match sdk {
        Sdk::Scala => std::fs::remove_dir_all(main.join("src"))?,
        Sdk::Go => std::fs::remove_dir_all(main.join("agents"))?,
        _ => {}
    }
    copy_inputs(&overlay, &main)?;
    let payload = std::fs::read_to_string(
        root.join("test-components/size-analysis/sdk-fixtures/payload.txt"),
    )?;
    // Scala.js IR hashes strings with writeUTF, whose encoded-string limit is 65,535 bytes.
    let payload = if sdk == Sdk::Scala {
        format!("{}\", \"{}", &payload[..50_000], &payload[50_000..])
    } else {
        payload
    };
    for entry in walkdir::WalkDir::new(&overlay) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let target = main.join(entry.path().strip_prefix(&overlay)?);
            let source = std::fs::read_to_string(&target)?;
            std::fs::write(target, source.replace("__SIZE_PAYLOAD__", &payload))?;
        }
    }
    match sdk {
        Sdk::Rust => {
            let manifest = main.join("Cargo.toml");
            let source = std::fs::read_to_string(&manifest)?;
            std::fs::write(
                manifest,
                source.replace("[dependencies]", "[dependencies]\nhex = \"=0.4.3\""),
            )?;
        }
        Sdk::Ts | Sdk::Effect => {
            let manifest = ctx.cwd_path_join("package.json");
            let mut package: Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
            package["dependencies"]["base64-js"] = "1.5.1".into();
            package["devDependencies"]["@types/base64-js"] = "1.5.0".into();
            std::fs::write(manifest, serde_json::to_vec_pretty(&package)?)?;
        }
        Sdk::Scala => {
            let build = ctx.cwd_path_join("size_fixture_main.sbt");
            let source = std::fs::read_to_string(&build)?;
            std::fs::write(build, format!("{}\n  .settings(Compile / unmanagedSourceDirectories += (LocalRootProject / baseDirectory).value / \"golem-temp/bridge-sdk/scala/internal/scala-streaming-tool-guest-client/src/main/scala\")\n", source.replace("libraryDependencies ++= Seq(", "libraryDependencies ++= Seq(\n      \"com.lihaoyi\" %%% \"ujson\" % \"3.1.0\",")))?;
        }
        Sdk::Go => {
            let manifest = main.join("go.mod");
            let source = std::fs::read_to_string(&manifest)?;
            let module = source
                .lines()
                .find_map(|line| line.strip_prefix("module "))
                .context("Go module missing")?;
            std::fs::write(
                main.join("main.go"),
                format!(
                    "package main\nimport (\n_ \"github.com/golemcloud/golem/sdks/go/golem\"\n_ \"{module}/fixture\"\n)\nfunc main() {{}}\n"
                ),
            )?;
            std::fs::write(
                manifest,
                format!(
                    "{source}\nrequire github.com/dustin/go-humanize v1.0.1\nrequire golem.local/bridge/stdin-probe-tool-guest-client v0.0.0\nreplace golem.local/bridge/stdin-probe-tool-guest-client => ../golem-temp/bridge-sdk/go/internal/stdin-probe-tool-guest-client\n"
                ),
            )?;
        }
        Sdk::Moonbit => {
            let manifest = ctx.cwd_path_join("moon.mod");
            let source = std::fs::read_to_string(&manifest)?;
            std::fs::write(
                manifest,
                source.replacen("import {", "import {\n  \"moonbitlang/x@0.5.1\",", 1),
            )?;
            let package = main.join("moon.pkg");
            let source = std::fs::read_to_string(&package)?;
            std::fs::write(package, source.replacen("import {", "import {\n  \"moonbit-streaming-tool-guest-client/client\" @provider,\n  \"golemcloud/golem_sdk/config\" @config,\n  \"moonbitlang/x/crypto\" @crypto,\n  \"moonbitlang/core/encoding/utf8\" @utf8,", 1))?;
        }
    }
    let manifest = ctx.cwd_path_join("golem.yaml");
    let mut yaml: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(&manifest)?)?;
    let provider = Fixture {
        sdk,
        kind: FixtureKind::Tool,
    }
    .load_descriptor(&root.join("test-components/size-analysis"))
    .map_err(anyhow::Error::msg)?;
    let CallTarget::Tool { tool, .. } = &provider.calls[0].target else {
        bail!("provider descriptor has no tool call")
    };
    if matches!(sdk, Sdk::Ts | Sdk::Effect) {
        let path = main.join("tsconfig.json");
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let name = format!("{tool}-tool-guest-client");
        let directory = format!("../golem-temp/bridge-sdk/{}/internal/{name}", sdk.name());
        config["compilerOptions"]["paths"][&name] =
            serde_json::json!([format!("{directory}/{name}.ts")]);
        config["include"]
            .as_array_mut()
            .context("TypeScript include list missing")?
            .push(format!("{directory}/*.ts").into());
        std::fs::write(path, serde_json::to_vec_pretty(&config)?)?;
    }
    yaml["components"]["size-fixture:main"]["dependencies"]["tools"] =
        serde_yaml::to_value([format!("size-fixture:provider/{tool}")])?;
    yaml["agents"]["CounterAgent"]["config"]["label"] = "fixture-configured".into();
    yaml["agents"]["CounterAgent"]["tools"][tool] = serde_yaml::Value::Mapping(Default::default());
    let app = yaml["app"]
        .as_str()
        .context("generated application has no name")?;
    yaml["httpApi"] = serde_yaml::from_str(&format!(
        "deployments:\n  local:\n    - domain: {app}.localhost:9006\n      agents:\n        CounterAgent: {{}}\n",
    ))?;
    std::fs::write(manifest, serde_yaml::to_string(&yaml)?)?;
    if matches!(sdk, Sdk::Moonbit | Sdk::Rust) {
        command(
            ctx,
            vec![
                flag::YES.into(),
                "-P".into(),
                preset.into(),
                cmd::BUILD.into(),
                "--step".into(),
                "gen-bridge".into(),
            ],
        )
        .await?;
    }
    if sdk == Sdk::Rust {
        let manifest = main.join("Cargo.toml");
        let source = std::fs::read_to_string(&manifest)?;
        std::fs::write(manifest, source.replace("[dependencies]", "[dependencies]\nstreaming-tool-guest-client = { path = \"../golem-temp/bridge-sdk/rust/internal/streaming-tool-guest-client\" }"))?;
    }
    if sdk == Sdk::Moonbit {
        super::add_moonbit_local_dependency(
            ctx,
            "moonbit-streaming-tool-guest-client",
            "golem-temp/bridge-sdk/moonbit/internal/moonbit-streaming-tool-guest-client",
        );
    }
    Ok(())
}

async fn sdk_fixtures(sdk: Sdk) {
    let mut ctx = TestContext::new();
    // Measurement fixtures must never depend on the caller's published-SDK test profile.
    let root = crate::workspace_path().canonicalize().unwrap();
    let mut overrides = SdkOverrides::default();
    overrides.golem_rust_path = Some(root.join("sdks/rust/golem-rust").display().to_string());
    overrides.ts_packages_path = Some(root.join("sdks/ts/packages").display().to_string());
    overrides.effect_golem_path = Some(root.join("sdks/effect").display().to_string());
    overrides.scala_sdk_version = Some("0.0.0-SNAPSHOT".into());
    overrides.moonbit_sdk_path = Some(root.join("sdks/moonbit/golem_sdk").display().to_string());
    overrides.go_sdk_path = Some(root.join("sdks/go/golem").display().to_string());
    ctx.env_mut().extend(overrides.to_env_vars());
    if sdk == Sdk::Rust {
        let target = ctx.data_dir.path().join("size-cargo-target");
        ctx.env_mut()
            .insert("CARGO_TARGET_DIR".into(), target.display().to_string());
    }
    if sdk == Sdk::Go {
        let toolchain = golem_cli::app::build::go_toolchain::ensure_go_toolchain(
            &golem_cli::model::app::ApplicationConfig {
                offline: false,
                dev_mode: false,
                should_colorize: false,
                enable_wasmtime_fs_cache: false,
            },
        )
        .await
        .expect("the Golem Go toolchain");
        let mut paths = vec![toolchain.bin_dir];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        ctx.env_mut().insert(
            "PATH".into(),
            std::env::join_paths(paths).unwrap().into_string().unwrap(),
        );
        ctx.env_mut().insert("GOTOOLCHAIN".into(), "local".into());
        // Absolute source paths otherwise make identical fixtures differ by directory.
        let flags = std::env::var("GOFLAGS").unwrap_or_default();
        ctx.env_mut()
            .insert("GOFLAGS".into(), format!("{flags} -trimpath"));
    }
    ctx.start_server().await;
    let parent = ctx.cwd_path().to_owned();
    let mut failures = Vec::new();
    let mut shared_runtime = std::collections::BTreeMap::new();
    for kind in [
        FixtureKind::StarterAgent,
        FixtureKind::Tool,
        FixtureKind::RepresentativeAgent,
    ] {
        let fixture = Fixture { sdk, kind };
        for preset in sdk.presets() {
            ctx.cd(&parent);
            let result: anyhow::Result<()> = async {
                let descriptor = fixture
                    .load_descriptor(&root.join("test-components/size-analysis"))
                    .map_err(anyhow::Error::msg)?;
                let app = format!("size-{}-{}-{preset}", sdk.name(), kind.name());
                let component = if kind == FixtureKind::Tool {
                    "size-fixture:provider"
                } else {
                    "size-fixture:main"
                };
                command(
                    &ctx,
                    vec![
                        flag::YES.into(),
                        cmd::NEW.into(),
                        app.clone(),
                        flag::TEMPLATE.into(),
                        sdk.name().into(),
                        flag::COMPONENT_NAME.into(),
                        component.into(),
                    ],
                )
                .await?;
                ctx.cd(&app);
                if kind == FixtureKind::Tool {
                    prepare_tool(&ctx, sdk)?;
                } else if kind == FixtureKind::RepresentativeAgent {
                    prepare_representative(&ctx, sdk, preset).await?;
                }
                if sdk == Sdk::Rust {
                    let manifest = ctx.cwd_path_join("Cargo.toml");
                    if manifest.exists() {
                        let source = std::fs::read_to_string(&manifest)?;
                        if !source.contains("[workspace]") {
                            std::fs::write(manifest, format!("{source}\n[workspace]\n"))?;
                        }
                    }
                }
                // Each application owns its domain; HTTP requests use Host without subdomain DNS.
                let manifest = ctx.cwd_path_join("golem.yaml");
                let yaml = std::fs::read_to_string(&manifest)?;
                std::fs::write(
                    manifest,
                    yaml.replace(
                        &format!("subdomain: {app}"),
                        &format!("domain: {app}.localhost:9006"),
                    ).replace(
                        "domain: localhost:9006",
                        &format!("domain: {app}.localhost:9006"),
                    ),
                )?;
                command(
                    &ctx,
                    vec![
                        flag::YES.into(),
                        "-P".into(),
                        preset.into(),
                        cmd::BUILD.into(),
                    ],
                )
                .await?;
                if kind != FixtureKind::StarterAgent {
                    let provider = "size-fixture:provider";
                    let path = selected_wasm(&ctx, preset, provider).await?;
                    let metadata = golem_cli::app::component_metadata::ComponentMetadataRegistry::new(true)
                        .get_or_extract_component_metadata(&golem_common::model::component::ComponentName(provider.into()), &path)
                        .await?;
                    let manifest = ctx.cwd_path_join("golem.yaml");
                    let mut yaml: serde_yaml::Value = serde_yaml::from_slice(&std::fs::read(&manifest)?)?;
                    // Source manifests can also declare definition-only clients for other providers.
                    yaml["tools"].as_mapping_mut().context("provider tools missing")?.retain(|name, _| {
                        name.as_str() == Some("middleware") || metadata.tools.iter().any(|tool| tool.name() == name.as_str())
                    });
                    if let CallTarget::Tool { tool, .. } = &descriptor.calls[0].target {
                        yaml["components"][provider]["tools"][tool] =
                            serde_yaml::Value::Mapping(Default::default());
                    }
                    std::fs::write(manifest, serde_yaml::to_string(&yaml)?)?;
                }
                if kind == FixtureKind::StarterAgent && matches!(sdk, Sdk::Ts | Sdk::Effect) {
                    let map = ctx.cwd_path_join(format!("golem-temp/ts-dist/{}/main.modules.json", component.to_kebab_case()));
                    let before = std::fs::read(&map)?;
                    std::fs::remove_file(&map)?;
                    command(&ctx, vec![flag::YES.into(), "-P".into(), preset.into(), cmd::BUILD.into()]).await?;
                    ensure!(std::fs::read(map)? == before, "an up-to-date build did not restore the module map");
                }
                command(
                    &ctx,
                    vec![
                        flag::YES.into(),
                        "-P".into(),
                        preset.into(),
                        cmd::DEPLOY.into(),
                    ],
                )
                .await?;
                if let Err(error) = source_resolution(&ctx, fixture, component).await {
                    failures.push(format!("{}/{preset}/source-resolution: {error:#}", fixture.series()));
                }
                match uploaded_wasm(&ctx, preset, component).await {
                    Ok((path, bytes)) => {
                        match collect_fixture(&ctx, fixture, preset, component, &path, &bytes).await {
                            Ok(runtime) if !runtime.0.is_empty() => {
                                if let Some(previous) = shared_runtime.insert(preset, runtime.clone())
                                    && previous != runtime {
                                        failures.push(format!("{}/{preset}/runtime: guest or runtime accounting differs across fixtures", fixture.series()));
                                    }
                            }
                            Ok(_) => {},
                            Err(error) => failures.push(format!(
                                "{}/{preset}/attribution: {error:#}",
                                fixture.series()
                            )),
                        }
                    }
                    Err(error) => {
                        failures.push(format!("{}/{preset}/upload: {error:#}", fixture.series()))
                    }
                }
                let bound = descriptor
                    .bind(&format!("{preset}-state"))
                    .map_err(anyhow::Error::msg)?;
                let first = checked_call(&ctx, &bound, &bound.calls[0])
                    .await
                    .context("first call")?;
                if let Err(error) = no_network(
                    &ctx,
                    component,
                    first
                        .1
                        .as_ref()
                        .context("first call has no oplog owner")?,
                )
                .await {
                    failures.push(format!("{}/{preset}/no-network: {error:#}", fixture.series()));
                }
                for (index, call) in bound.calls.iter().enumerate().skip(1) {
                    if let Err(error) = checked_call(&ctx, &bound, call).await {
                        failures.push(format!(
                            "{}/{preset}/call-{index}: {error:#}",
                            fixture.series()
                        ));
                    }
                }
                let fresh = descriptor
                    .bind(&format!("{preset}-fresh"))
                    .map_err(anyhow::Error::msg)?;
                let repeated = checked_call(&ctx, &fresh, &fresh.calls[0])
                    .await
                    .context("fresh equivalent first call")?;
                ensure!(first.0 == repeated.0, "fresh equivalent first calls differ");
                no_network(
                    &ctx,
                    component,
                    repeated
                        .1
                        .as_ref()
                        .context("fresh first call has no oplog owner")?,
                )
                .await?;
                if kind == FixtureKind::StarterAgent
                    && let Err(error) = unused_import(&ctx, sdk, preset, component).await {
                        failures.push(format!("{}/{preset}/unused-import: {error:#}", fixture.series()));
                    }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                failures.push(format!(
                    "{}/{preset}: {error:#}; dependent checks blocked",
                    fixture.series()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[timeout("60 minutes")]
async fn ts_starter() {
    sdk_fixtures(Sdk::Ts).await;
}

#[test]
#[timeout("60 minutes")]
async fn effect_starter() {
    sdk_fixtures(Sdk::Effect).await;
}

#[test]
#[timeout("60 minutes")]
async fn scala_starter() {
    sdk_fixtures(Sdk::Scala).await;
}

#[test]
#[timeout("60 minutes")]
async fn rust_starter() {
    sdk_fixtures(Sdk::Rust).await;
}

#[test]
#[timeout("60 minutes")]
async fn moonbit_starter() {
    sdk_fixtures(Sdk::Moonbit).await;
}

#[test]
#[timeout("60 minutes")]
async fn go_starter() {
    sdk_fixtures(Sdk::Go).await;
}

#[test]
fn first_call_oplog_requires_complete_network_free_evidence() {
    use golem_cli::model::agent::oplog::OmittedOplogEntryContent;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::caller::{Caller, ClaimVisibility, PublicCaller};
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::host_functions::WebsocketClientConnect;
    use golem_common::model::oplog::payload::HostPayloadPair;
    use golem_common::model::oplog::public_oplog_entry::{
        AgentInvocationFinishedParams, AgentInvocationStartedParams, CreateParams, StartParams,
    };
    use golem_common::model::oplog::{
        ProcessOplogEntriesParameters, ProcessOplogEntriesResultParameters,
        PublicAgentInvocationResult, PublicDurableFunctionType, PublicOplogEntryAttribution,
    };
    use golem_common::model::{AgentId, InvocationId, Timestamp};
    use golem_schema::Empty;

    let id = InvocationId(uuid::Uuid::from_u128(1));
    let wrap = |index, entry| AgentOplogEntryView {
        index,
        attribution: PublicOplogEntryAttribution::agent(),
        entry: AgentOplogEntryContent::Full(entry),
    };
    let entries = vec![
        wrap(
            1,
            PublicOplogEntry::Create(CreateParams {
                timestamp: Timestamp::from(0),
                agent_id: AgentId::from_parts(0, 2),
                agent_type_name: None,
                owner_kind: Default::default(),
                agent_mode: AgentMode::Durable,
                component_revision: ComponentRevision::INITIAL,
                env: Default::default(),
                created_by: AccountId::new(),
                environment_id: EnvironmentId::new(),
                parent: None,
                component_size: 1,
                initial_total_linear_memory_size: 0,
                initial_active_plugins: Default::default(),
                local_agent_config: vec![],
                instance_id: uuid::Uuid::from_u128(3),
                never_live: false,
                standalone_tool: None,
            }),
        ),
        wrap(
            2,
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                timestamp: Timestamp::from(0),
                invocation: PublicAgentInvocation::ProcessOplogEntries(
                    ProcessOplogEntriesParameters {
                        invocation_id: id,
                        caller: PublicCaller::new(&Caller::anonymous(), ClaimVisibility::Hidden),
                    },
                ),
                wallet_pin: golem_common::model::card::PublicInvocationWalletPin {
                    wallet_token: golem_common::model::card::WalletVersionToken {
                        wallet_id_hash: [0; 32],
                        generation: 0,
                    },
                    scope_card_id: None,
                },
            }),
        ),
        wrap(
            3,
            PublicOplogEntry::Start(StartParams {
                timestamp: Timestamp::from(0),
                parent_start_index: None,
                function_name: "cli::environment::get-environment".into(),
                invocation_id: None,
                observational_owner: None,
                driver: None,
                request: None,
                durable_function_type: PublicDurableFunctionType::ReadLocal(Empty {}),
                span_started: None,
                denial: None,
            }),
        ),
        wrap(
            4,
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: Timestamp::from(0),
                invocation_id: id,
                result: PublicAgentInvocationResult::ProcessOplogEntries(
                    ProcessOplogEntriesResultParameters { error: None },
                ),
                method_name: None,
                consumed_fuel: 0,
                component_revision: ComponentRevision::INITIAL,
            }),
        ),
    ];
    check_no_network(&entries).unwrap();
    assert!(check_no_network(&[]).is_err());
    assert!(check_no_network(&entries[1..]).is_err());
    assert!(check_no_network(&entries[..3]).is_err());

    for function in [
        "http::outgoing-handler::handle",
        "sockets::tcp::permissions::check-connect",
        "sockets::ip-name-lookup::resolve-addresses",
        "wasi:http/outgoing-handler.handle",
        "wasi:sockets/tcp.start-connect",
        "golem::network::connect",
        "rdbms::postgres::db-connection::query",
        "rdbms::mysql::db-connection::query",
        golem_common::model::oplog::host_functions::RdbmsPostgresDbConnectionQuery::FQFN,
        golem_common::model::oplog::host_functions::RdbmsMysqlDbConnectionQuery::FQFN,
        WebsocketClientConnect::FQFN,
    ] {
        let mut network = entries.clone();
        let AgentOplogEntryContent::Full(PublicOplogEntry::Start(call)) = &mut network[2].entry
        else {
            unreachable!()
        };
        call.function_name = function.into();
        call.durable_function_type = PublicDurableFunctionType::WriteRemote(Empty {});
        assert!(check_no_network(&network).is_err(), "{function}");
    }
    let mut gap = entries.clone();
    gap[2].index += 1;
    assert!(check_no_network(&gap).is_err());
    let mut omitted = entries.clone();
    omitted[2].entry = AgentOplogEntryContent::Omitted(OmittedOplogEntryContent::Omitted {
        entry_type: "Start".into(),
        timestamp: Timestamp::from(0),
        size: 1,
    });
    assert!(check_no_network(&omitted).is_err());
    let mut mismatched = entries;
    let AgentOplogEntryContent::Full(PublicOplogEntry::AgentInvocationFinished(call)) =
        &mut mismatched[3].entry
    else {
        unreachable!()
    };
    call.invocation_id = InvocationId(uuid::Uuid::from_u128(99));
    assert!(check_no_network(&mismatched).is_err());
}
