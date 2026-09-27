use super::http_test_context::{HttpTestContext, make_test_context_with_files};
use golem_common::agent_id;
use golem_common::data_value;
use golem_common::model::AgentId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDsl;
use golem_test_framework::model::IFSEntry;
use reqwest::{Method, StatusCode};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(EnvBasedTestDependencies);

fn large_file_contents() -> Vec<u8> {
    let mut contents = vec![0; 32 * 1024 * 1024];
    blake3::Hasher::new()
        .update(b"live-file-backpressure")
        .finalize_xof()
        .fill(&mut contents);
    contents
}

#[test]
fn large_file_exceeds_compressed_grpc_window() {
    use flate2::{Compression, write::GzEncoder};
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        GetFileContentsResponse, get_file_contents_response,
    };
    use prost::Message;
    use std::io::Write;

    let wire_size = |contents: &[u8]| -> usize {
        contents
            .chunks(golem_common::model::filesystem::FILE_READ_CHUNK_SIZE)
            .map(|chunk| {
                let message = GetFileContentsResponse {
                    result: Some(get_file_contents_response::Result::Success(chunk.to_vec())),
                };
                let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
                gzip.write_all(&message.encode_to_vec()).unwrap();
                5 + gzip.finish().unwrap().len()
            })
            .sum()
    };
    // The old periodic payload fits inside the inner 2 MiB HTTP/2 receive window,
    // allowing the executor to reach EOF despite a backpressured HTTP response.
    let periodic: Vec<u8> = (0..32 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    assert!(wire_size(&periodic) < 2 * 1024 * 1024);
    assert!(wire_size(&large_file_contents()) > 30 * 1024 * 1024);
}

async fn context(deps: &EnvBasedTestDependencies) -> anyhow::Result<HttpTestContext> {
    make_test_context_with_files(
        deps,
        vec![(
            AgentTypeName("LiveFiles".into()),
            HttpApiDeploymentAgentOptions::default(),
        )],
        "it_initial_file_system_release",
        "golem-it:initial-file-system",
        HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
        &[(
            "LiveFiles",
            vec![
                IFSEntry {
                    source_path: "initial-file-system/files/foo.txt".into(),
                    target_path: CanonicalFilePath::from_abs_str("/public/ro.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: "initial-file-system/files/baz.txt".into(),
                    target_path: CanonicalFilePath::from_abs_str("/public/rw.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )],
    )
    .await
}

fn agent(context: &HttpTestContext, name: &str) -> AgentId {
    AgentId {
        component_id: context.component_id,
        agent_id: agent_id!("LiveFiles", name).to_string(),
    }
}

async fn effects(context: &HttpTestContext, name: &str) -> anyhow::Result<(usize, usize)> {
    let mut counts = (0, 0);
    for entry in context
        .user
        .get_oplog(&agent(context, name), OplogIndex::INITIAL)
        .await?
    {
        if let PublicOplogEntry::AgentInvocationStarted(start) = entry.entry {
            match start.invocation {
                PublicAgentInvocation::AgentInitialization(_) => counts.0 += 1,
                PublicAgentInvocation::AgentMethodInvocation(_) => counts.1 += 1,
                _ => {}
            }
        }
    }
    Ok(counts)
}

#[test]
#[timeout("180s")]
async fn mounted_live_files_initialize_once_and_serve_current_files(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let context = context(deps).await?;
    assert!(
        context
            .user
            .get_worker_metadata_opt(&agent(&context, "first"))
            .await?
            .is_none()
    );
    let url = context.base_url.join("/files/first/created.txt")?;
    let (first, concurrent) = tokio::join!(
        context.client.get(url.clone()).send(),
        context.client.get(url).send(),
    );
    for response in [first?, concurrent?] {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await?, b"first".as_slice());
    }
    assert_eq!(effects(&context, "first").await?, (1, 0));
    for (method, suffix, headers, status, expected) in [
        (
            Method::GET,
            "first/ro.txt",
            vec![],
            200,
            b"foo\n".as_slice(),
        ),
        (
            Method::GET,
            "first/rw.txt",
            vec![],
            200,
            b"baz\n".as_slice(),
        ),
        (
            Method::GET,
            "second/alias",
            vec![],
            200,
            b"second".as_slice(),
        ),
        (
            Method::GET,
            "first/only.txt",
            vec![],
            200,
            b"fallback".as_slice(),
        ),
        (
            Method::HEAD,
            "first/created.txt",
            vec![("range", "bytes=1-2")],
            200,
            b"".as_slice(),
        ),
        (
            Method::GET,
            "first/created.txt",
            vec![("range", "bytes=1-3")],
            206,
            b"irs".as_slice(),
        ),
        (
            Method::GET,
            "first/created.txt",
            vec![("if-none-match", "*")],
            304,
            b"".as_slice(),
        ),
        (
            Method::GET,
            "first/created.txt",
            vec![("range", "bytes=9-")],
            416,
            b"".as_slice(),
        ),
        (Method::GET, "first/directory", vec![], 403, b"".as_slice()),
        (
            Method::GET,
            "first/created.txt/",
            vec![],
            403,
            b"".as_slice(),
        ),
        (Method::GET, "first/link.txt", vec![], 403, b"".as_slice()),
        (
            Method::GET,
            "first/parent-link/secret.txt",
            vec![],
            403,
            b"".as_slice(),
        ),
        (
            Method::GET,
            "first/private/secret.txt",
            vec![],
            404,
            b"".as_slice(),
        ),
    ] {
        let mut request = context
            .client
            .request(method, context.base_url.join(&format!("/files/{suffix}"))?)
            .header("origin", "https://allowed.test");
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request.send().await?;
        assert_eq!(response.status().as_u16(), status, "{suffix}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://allowed.test"
        );
        assert!(!response.headers().contains_key("etag"));
        assert!(!response.headers().contains_key("last-modified"));
        assert_eq!(response.bytes().await?, expected, "{suffix}");
    }
    assert_eq!(effects(&context, "second").await?, (1, 0));
    let component = context
        .user
        .get_latest_component_revision(&context.component_id)
        .await?;
    assert!(
        !context
            .user
            .invoke_and_await_agent(
                &component,
                &agent_id!("LiveFiles", "first"),
                "replace",
                data_value!("/public/ro.txt", b"forbidden".to_vec()),
            )
            .await?
            .into_typed::<bool>()?
    );
    let protected = context
        .client
        .get(context.base_url.join("/files/first/ro.txt")?)
        .send()
        .await?;
    assert_eq!(protected.status(), StatusCode::OK);
    assert_eq!(protected.bytes().await?, b"foo\n".as_slice());
    for (path, contents) in [
        ("rw.txt", b"changed".as_slice()),
        ("dynamic.txt", b"new file".as_slice()),
    ] {
        assert!(
            context
                .user
                .invoke_and_await_agent(
                    &component,
                    &agent_id!("LiveFiles", "first"),
                    "replace",
                    data_value!(format!("/public/{path}"), contents.to_vec()),
                )
                .await?
                .into_typed::<bool>()?
        );
        let response = context
            .client
            .get(context.base_url.join(&format!("/files/first/{path}"))?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await?, contents);
    }
    assert_eq!(effects(&context, "first").await?, (1, 3));
    context
        .user
        .simulated_crash(&agent(&context, "first"))
        .await?;
    let restored = context
        .client
        .get(context.base_url.join("/files/first/dynamic.txt")?)
        .send()
        .await?;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(restored.bytes().await?, b"new file".as_slice());
    assert_eq!(effects(&context, "first").await?, (1, 3));
    context
        .user
        .revert(
            &agent(&context, "first"),
            golem_common::model::worker::RevertWorkerTarget::RevertLastInvocations(
                golem_common::model::worker::RevertLastInvocations {
                    number_of_invocations: 1,
                },
            ),
        )
        .await?;
    let reverted = context
        .client
        .get(context.base_url.join("/files/first/dynamic.txt")?)
        .send()
        .await?;
    assert_eq!(reverted.status(), StatusCode::NOT_FOUND);
    let retained = context
        .client
        .get(context.base_url.join("/files/first/rw.txt")?)
        .send()
        .await?;
    assert_eq!(retained.bytes().await?, b"changed".as_slice());
    let response = context
        .client
        .get(context.base_url.join("/files/fail/created.txt")?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!response.text().await?.contains("private"));
    Ok(())
}

#[test]
#[timeout("180s")]
async fn mounted_live_stream_completion_and_disconnect_release_mutation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let context = context(deps).await?;
    let component = context
        .user
        .get_latest_component_revision(&context.component_id)
        .await?;
    let expected = large_file_contents();
    for disconnect in [false, true] {
        let name = format!("large-{disconnect}");
        let domain = context.host_header.to_str()?;
        let address = ([127, 0, 0, 1], context.base_url.port().unwrap()).into();
        // Bound outer transport read-ahead as well as the gzip-compressed inner gRPC
        // stream. The fixture must remain larger than both windows after compression.
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve(domain, address)
            .timeout(Duration::from_secs(60))
            .http2_prior_knowledge()
            .http2_initial_stream_window_size(64 * 1024)
            .build()?;
        let url = reqwest::Url::parse(&format!("http://{domain}/files/{name}/large.bin"))?;
        let mut response = client.get(url.clone()).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-length"],
            (32 * 1024 * 1024).to_string()
        );
        let first = response.chunk().await?.unwrap();
        assert_eq!(first.as_ref(), &expected[..first.len()]);
        let parsed = agent_id!("LiveFiles", name.as_str());
        let mut mutation = Box::pin(context.user.invoke_and_await_agent(
            &component,
            &parsed,
            "replace",
            data_value!("/public/large.bin", b"after".to_vec()),
        ));
        tokio::select! {
            result = &mut mutation => panic!("mutation passed an active file read: {result:?}"),
            admitted = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let oplog = context.user.get_oplog(&agent(&context, &name), OplogIndex::INITIAL).await?;
                    if oplog.iter().any(|entry| matches!(&entry.entry,
                        PublicOplogEntry::PendingAgentInvocation(pending)
                            if matches!(pending.invocation, PublicAgentInvocation::AgentMethodInvocation(_)))) {
                        break anyhow::Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }) => admitted??,
        }
        let early_mutation = tokio::time::timeout(Duration::from_millis(200), &mut mutation).await;
        assert!(
            early_mutation.is_err(),
            "mutation completed while the response was held (disconnect={disconnect}): {early_mutation:?}"
        );
        if disconnect {
            drop(response);
        } else {
            let mut offset = first.len();
            while let Some(chunk) = response.chunk().await? {
                assert_eq!(chunk.as_ref(), &expected[offset..offset + chunk.len()]);
                offset += chunk.len();
            }
            assert_eq!(offset, 32 * 1024 * 1024);
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(20), mutation)
                .await??
                .into_typed::<bool>()?
        );
        let response = client.get(url).send().await?;
        assert_eq!(response.bytes().await?, b"after".as_slice());
        assert_eq!(effects(&context, &name).await?, (1, 1));
    }
    Ok(())
}
