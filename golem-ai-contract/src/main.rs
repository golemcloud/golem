// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use golem_ai_contract::{
    BodyFormat, CREDENTIAL, Corpus, Operation, Provenance, Recording, Request, Response,
    check_corpus, contract, json_bytes, normalize, openai_faults,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(about = "Manual AI wire recorder and offline corpus checker")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check coverage, declared IDs, normalization and credential redaction. No network.
    Check { directory: PathBuf },
    /// Check all indexed contract assets and recording sets. No network or writes.
    CheckContract { directory: PathBuf },
    /// Make live OpenAI calls. Reads OPENAI_API_KEY; never run in CI.
    RecordOpenai {
        /// Must not exist. No raw captures are written to disk.
        directory: PathBuf,
        #[arg(long, default_value = "gpt-4.1-mini")]
        model: String,
        #[arg(long, default_value = "text-embedding-3-small")]
        embedding_model: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Check { directory } => check_corpus(&directory),
        Command::CheckContract { directory } => contract::check(&directory),
        Command::RecordOpenai {
            directory,
            model,
            embedding_model,
        } => {
            ensure!(
                std::env::var_os("CI").is_none(),
                "live recording is forbidden in CI"
            );
            ensure!(
                !directory.exists(),
                "record into a new directory, then review the diff"
            );
            let key = std::env::var("OPENAI_API_KEY")
                .context("set OPENAI_API_KEY for manual recording")?;
            ensure!(!key.is_empty(), "OPENAI_API_KEY is empty");
            record_openai(directory, &key, &model, &embedding_model).await
        }
    }
}

async fn record_openai(
    directory: PathBuf,
    key: &str,
    model: &str,
    embedding_model: &str,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()?;
    let mut files = BTreeMap::new();
    let mut corpus = Corpus {
        case_ids: Vec::new(),
        cases: Vec::new(),
        operations: Vec::new(),
    };
    for (operation, path, body, streaming) in [
        (
            "chat",
            "chat/completions",
            json!({"model":model,"messages":[{"role":"user","content":"Say hello in one short sentence."}],"max_completion_tokens":64}),
            true,
        ),
        (
            "structured-output",
            "chat/completions",
            json!({"model":model,"messages":[{"role":"user","content":"Return the city Paris and the country France."}],"max_completion_tokens":128,"response_format":{"type":"json_schema","json_schema":{"name":"place","strict":true,"schema":{"type":"object","properties":{"city":{"type":"string"},"country":{"type":"string"}},"required":["city","country"],"additionalProperties":false}}}}),
            true,
        ),
        (
            "tool-calls",
            "chat/completions",
            json!({"model":model,"messages":[{"role":"user","content":"Look up the weather in Budapest."}],"max_completion_tokens":128,"tools":[{"type":"function","function":{"name":"weather","description":"Look up weather in a city","strict":true,"parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}}}],"tool_choice":{"type":"function","function":{"name":"weather"}}}),
            true,
        ),
        (
            "embed",
            "embeddings",
            json!({"model":embedding_model,"input":["A short sentence.","A different sentence."],"dimensions":8,"encoding_format":"float"}),
            false,
        ),
    ] {
        corpus.operations.push(Operation {
            provider: "openai".into(),
            operation: operation.into(),
            streaming,
        });
        for stream in if streaming {
            vec![false, true]
        } else {
            vec![false]
        } {
            let mut body = body.clone();
            if stream {
                body["stream"] = json!(true);
                body["stream_options"] = json!({"include_usage":true});
            }
            let id = format!(
                "openai-{operation}-{}",
                if stream { "stream" } else { "plain" }
            );
            let format = if stream {
                BodyFormat::Sse
            } else {
                BodyFormat::Json
            };
            let (recording, bytes) =
                capture(&client, key, &id, operation, path, body, 200, format).await?;
            add(&mut corpus, &mut files, recording, bytes)?;
        }
        let id = format!("openai-{operation}-401");
        let (recording, bytes) = capture(
            &client,
            "invalid-corpus-key",
            &id,
            operation,
            path,
            body.clone(),
            401,
            BodyFormat::Json,
        )
        .await?;
        add(&mut corpus, &mut files, recording, bytes)?;
        // Do not exhaust a real account or induce outages to record server faults.
        // Their provenance remains distinct from the live responses above.
        for fault in openai_faults()? {
            let id = format!("openai-{operation}-{}", fault.id);
            let mut recording = Recording {
                id: id.clone(),
                provider: "openai".into(),
                operation: operation.into(),
                provenance: Provenance::FaultInjection,
                request: request(path, body.clone()),
                response: Response {
                    status: fault.status,
                    headers: fault.headers,
                    body: format!("{id}.body"),
                    format: fault.format,
                },
            };
            let bytes = normalize(&mut recording, fault.body.as_bytes())?;
            add(&mut corpus, &mut files, recording, bytes)?;
        }
    }
    files.insert("index.json".into(), json_bytes(&corpus)?);
    // Only normalized bytes reach the filesystem, including error bodies which may
    // echo a credential. The known key is scrubbed before parsing in capture().
    if let Some(parent) = directory.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&directory).context("recording directory must not already exist")?;
    for (file, bytes) in files {
        std::fs::write(directory.join(file), bytes)?;
    }
    check_corpus(&directory)
}

fn request(path: &str, body: Value) -> Request {
    Request {
        method: "POST".into(),
        url: format!("https://api.openai.com/v1/{path}"),
        headers: BTreeMap::from([
            ("authorization".into(), CREDENTIAL.into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body,
    }
}

#[allow(clippy::too_many_arguments)]
async fn capture(
    client: &reqwest::Client,
    key: &str,
    id: &str,
    operation: &str,
    path: &str,
    body: Value,
    expected: u16,
    format: BodyFormat,
) -> Result<(Recording, Vec<u8>)> {
    let request = request(path, body);
    let mut response = client
        .post(&request.url)
        .bearer_auth(key)
        .json(&request.body)
        .send()
        .await
        .with_context(|| format!("recording {id}"))?;
    ensure!(
        response.status().as_u16() == expected,
        "{id}: expected HTTP {expected}, got {}; response body withheld",
        response.status()
    );
    // Retain only protocol-relevant headers, not volatile account/rate-limit metadata.
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type" | "retry-after" | "date" | "x-request-id"
            )
        })
        .map(|(name, value)| Ok((name.to_string(), value.to_str()?.replace(key, CREDENTIAL))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 16 * 1024 * 1024,
            "{id}: response exceeds 16 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let safe_body = std::str::from_utf8(&bytes)?.replace(key, CREDENTIAL);
    let mut recording = Recording {
        id: id.into(),
        provider: "openai".into(),
        operation: operation.into(),
        provenance: Provenance::Live,
        request,
        response: Response {
            status: expected,
            headers,
            body: format!("{id}.body"),
            format,
        },
    };
    let bytes = normalize(&mut recording, safe_body.as_bytes())?;
    Ok((recording, bytes))
}

fn add(
    corpus: &mut Corpus,
    files: &mut BTreeMap<String, Vec<u8>>,
    recording: Recording,
    body: Vec<u8>,
) -> Result<()> {
    let file = format!("{}.json", recording.id);
    corpus.case_ids.push(recording.id.clone());
    corpus.cases.push(file.clone());
    files.insert(file, json_bytes(&recording)?);
    files.insert(recording.response.body, body);
    Ok(())
}
