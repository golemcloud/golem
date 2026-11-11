// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

//! Recording format and credential-safe normalization for the shared AI corpus.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub mod contract;

#[cfg(test)]
test_r::enable!();

pub const CREDENTIAL: &str = "<credential>";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    /// A sibling file. SSE is stored as bytes, not parsed events or a JSON string.
    pub body: String,
    pub format: BodyFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyFormat {
    Json,
    Sse,
    Malformed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    Live,
    /// Deliberately invalid wire data; never represented as a provider response.
    FaultInjection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recording {
    pub id: String,
    pub provider: String,
    pub operation: String,
    pub provenance: Provenance,
    pub request: Request,
    pub response: Response,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Corpus {
    pub case_ids: Vec<String>,
    pub cases: Vec<String>,
    /// Explicit coverage requirements, so removing a case cannot silently weaken the suite.
    pub operations: Vec<Operation>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub provider: String,
    pub operation: String,
    /// False for APIs, such as embeddings, which have no stream protocol.
    pub streaming: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectedFault {
    pub id: String,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub format: BodyFormat,
    pub body: String,
    pub expected: Value,
}

pub fn openai_faults() -> Result<Vec<InjectedFault>> {
    let value: Value =
        serde_json::from_str(include_str!("../../sdks/ai-contract/openai-faults.json"))?;
    serde_json::from_value(value["cases"].clone()).context("invalid OpenAI fault fixtures")
}

pub fn credential_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "xi-api-key"
            | "x-goog-api-key"
            | "ocp-apim-subscription-key"
            | "x-amz-security-token"
            | "cookie"
            | "set-cookie"
    )
}

pub fn normalize_headers(headers: &mut BTreeMap<String, String>) {
    *headers = std::mem::take(headers)
        .into_iter()
        .map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            let value = if credential_header(&name) {
                CREDENTIAL.to_string()
            } else if name == "date" || name == "last-modified" || name == "expires" {
                "<date>".to_string()
            } else if name.contains("request-id") || name.contains("trace-id") {
                "<id>".to_string()
            } else {
                value
            };
            (name, value)
        })
        .collect();
}

pub fn normalize_url(raw: &str) -> Result<String> {
    let mut url = url::Url::parse(raw).context("invalid recording URL")?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URL user info is not supported in recordings"
    );
    let pairs: Vec<_> = url
        .query_pairs()
        .map(|(name, value)| {
            let value = if matches!(
                name.to_ascii_lowercase().as_str(),
                "key" | "api_key" | "api-key" | "access_token"
            ) {
                CREDENTIAL.to_string()
            } else {
                value.into_owned()
            };
            (name.into_owned(), value)
        })
        .collect();
    if url.query().is_some() {
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    Ok(url.to_string())
}

/// Normalize only response metadata. Model names, usage and generated content stay intact.
/// A single normalizer covers all chunks of a response, preserving distinct tool-call IDs.
#[derive(Default)]
pub struct Normalizer {
    ids: BTreeMap<String, String>,
}

impl Normalizer {
    pub fn json(&mut self, value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    match name.as_str() {
                        "id" | "tool_call_id" if value.is_string() => {
                            let original = value.as_str().unwrap();
                            if !original.starts_with("<id:") {
                                let next = self.ids.len() + 1;
                                let replacement = self
                                    .ids
                                    .entry(original.to_string())
                                    .or_insert_with(|| format!("<id:{next}>"));
                                *value = Value::String(replacement.clone());
                            }
                        }
                        "created" | "created_at" | "date" => {
                            *value = Value::String("<date>".to_string())
                        }
                        "system_fingerprint" => *value = Value::String("<fingerprint>".to_string()),
                        _ => self.json(value),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.json(item)),
            _ => {}
        }
    }

    pub fn body(&mut self, bytes: &[u8], format: BodyFormat) -> Result<Vec<u8>> {
        match format {
            BodyFormat::Json => {
                let mut value: Value =
                    serde_json::from_slice(bytes).context("invalid JSON response")?;
                self.json(&mut value);
                json_bytes(&value)
            }
            BodyFormat::Sse => {
                let mut output = Vec::new();
                // Preserve framing, CRLF and the terminal sentinel. OpenAI sends one JSON
                // value per data line; do not flatten it into an array of parsed events.
                for line in bytes.split_inclusive(|byte| *byte == b'\n') {
                    let ending = if line.ends_with(b"\r\n") {
                        b"\r\n".as_slice()
                    } else if line.ends_with(b"\n") {
                        b"\n".as_slice()
                    } else {
                        b"".as_slice()
                    };
                    let content = &line[..line.len() - ending.len()];
                    if let Some(data) = content.strip_prefix(b"data:") {
                        let data = data.strip_prefix(b" ").unwrap_or(data);
                        if data != b"[DONE]" && !data.is_empty() {
                            let mut value: Value = serde_json::from_slice(data)
                                .context("invalid SSE JSON data line")?;
                            self.json(&mut value);
                            output.extend_from_slice(b"data: ");
                            output.extend_from_slice(&serde_json::to_vec(&value)?);
                        } else {
                            output.extend_from_slice(content);
                        }
                    } else {
                        output.extend_from_slice(content);
                    }
                    output.extend_from_slice(ending);
                }
                Ok(output)
            }
            BodyFormat::Malformed => Ok(bytes.to_vec()),
        }
    }
}

pub fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn normalize(recording: &mut Recording, body: &[u8]) -> Result<Vec<u8>> {
    recording.request.url = normalize_url(&recording.request.url)?;
    normalize_headers(&mut recording.request.headers);
    normalize_headers(&mut recording.response.headers);
    Normalizer::default().body(body, recording.response.format)
}

fn sibling(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    ensure!(
        path.components().count() == 1
            && matches!(path.components().next(), Some(Component::Normal(_))),
        "corpus paths must be sibling filenames"
    );
    Ok(path)
}

pub fn check_corpus(directory: &Path) -> Result<()> {
    let corpus: Corpus = serde_json::from_slice(&std::fs::read(directory.join("index.json"))?)?;
    ensure!(
        !corpus.operations.is_empty(),
        "corpus declares no operations"
    );
    let declared: BTreeSet<_> = corpus.case_ids.iter().cloned().collect();
    ensure!(
        declared.len() == corpus.case_ids.len(),
        "duplicate declared case ID"
    );
    let mut actual = BTreeSet::new();
    let mut files = BTreeSet::from(["index.json".to_string()]);
    let mut coverage: BTreeMap<(String, String), (bool, bool, bool)> = BTreeMap::new();
    for file in &corpus.cases {
        ensure!(files.insert(file.clone()), "duplicate recording filename");
        let path = directory.join(sibling(file)?);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut recording: Recording = serde_json::from_slice(&bytes)?;
        ensure!(
            actual.insert(recording.id.clone()),
            "duplicate recorded case ID"
        );
        ensure!(
            (200..600).contains(&recording.response.status),
            "invalid response status"
        );
        let body_path = sibling(&recording.response.body)?;
        ensure!(
            files.insert(recording.response.body.clone()),
            "duplicate body filename"
        );
        let body = std::fs::read(directory.join(body_path))?;
        let normalized = normalize(&mut recording, &body)?;
        ensure!(
            json_bytes(&recording)? == bytes && normalized == body,
            "recording is not normalized: {file}"
        );
        ensure!(!body.is_empty(), "empty response body: {file}");
        if recording.response.status < 400 && recording.response.format != BodyFormat::Malformed {
            ensure!(
                recording.provenance == Provenance::Live,
                "success response must have live provenance: {file}"
            );
        }
        let parts = coverage
            .entry((recording.provider.clone(), recording.operation.clone()))
            .or_default();
        if recording.response.status >= 400 || recording.response.format == BodyFormat::Malformed {
            parts.2 = true;
        } else if recording.response.format == BodyFormat::Sse {
            if recording.provider == "openai" {
                let mut has_data = false;
                let mut done = false;
                for line in std::str::from_utf8(&body)?.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.strip_prefix(' ').unwrap_or(data);
                        if data.is_empty() {
                            continue;
                        }
                        ensure!(!done, "data after OpenAI terminal sentinel: {file}");
                        if data == "[DONE]" {
                            done = true;
                        } else {
                            has_data = true;
                        }
                    }
                }
                ensure!(has_data && done, "incomplete OpenAI stream: {file}");
            }
            parts.1 = true;
        } else {
            parts.0 = true;
        }
    }
    ensure!(
        actual == declared,
        "recorded cases differ from declared case-ID set"
    );
    let mut operations = BTreeSet::new();
    for operation in corpus.operations {
        let key = (operation.provider, operation.operation);
        ensure!(
            operations.insert(key.clone()),
            "duplicate operation declaration"
        );
        let Some((plain, stream, error)) = coverage.remove(&key) else {
            bail!("missing recordings for {key:?}")
        };
        ensure!(
            plain && error && (!operation.streaming || stream),
            "incomplete plain/stream/error coverage for {key:?}"
        );
    }
    ensure!(
        coverage.is_empty(),
        "recording has an undeclared provider operation"
    );
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(
            files.remove(&entry.file_name().to_string_lossy().to_string()),
            "unlisted corpus file: {}",
            entry.path().display()
        );
    }
    ensure!(files.is_empty(), "missing corpus files");
    Ok(())
}

#[cfg(test)]
mod tests;
