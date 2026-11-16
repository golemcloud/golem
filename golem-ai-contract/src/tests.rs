// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use serde_json::json;
use test_r::test;

#[test]
fn normalization_is_idempotent_and_keeps_wire_facts() {
    let raw = b"event: message\r\ndata: {\"id\":\"completion-a\",\"created\":1987654321,\"system_fingerprint\":\"fp-secret\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call-a\"},{\"id\":\"call-b\"}]}}]}\r\n\r\ndata: {\"id\":\"completion-a\",\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\r\n\r\ndata: [DONE]\r\n\r\n";
    let once = Normalizer::default().body(raw, BodyFormat::Sse).unwrap();
    assert_eq!(
        once,
        Normalizer::default().body(&once, BodyFormat::Sse).unwrap()
    );
    let text = String::from_utf8(once).unwrap();
    assert!(text.starts_with("event: message\r\ndata: "));
    assert!(text.ends_with("data: [DONE]\r\n\r\n"));
    let chunks: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(chunks[0]["id"], chunks[1]["id"]);
    let calls = chunks[0]["choices"][0]["delta"]["tool_calls"]
        .as_array()
        .unwrap();
    assert_ne!(calls[0]["id"], calls[1]["id"]);
    assert_ne!(calls[0]["id"], chunks[0]["id"]);
    assert!(!text.contains("completion-a") && !text.contains("call-a"));
    assert!(text.contains("<date>") && text.contains("<fingerprint>"));
    assert!(text.contains("\"prompt_tokens\":7") && text.contains("\"completion_tokens\":3"));
}

#[test]
fn credentials_are_redacted_case_insensitively_in_headers_and_queries() {
    let mut headers = BTreeMap::from([
        ("Authorization".into(), "Bearer private".into()),
        ("X-Goog-Api-Key".into(), "private".into()),
        ("Date".into(), "Tue, 10 Nov 2026 10:00:00 GMT".into()),
        ("Retry-After".into(), "17".into()),
    ]);
    normalize_headers(&mut headers);
    assert_eq!(headers["authorization"], CREDENTIAL);
    assert_eq!(headers["x-goog-api-key"], CREDENTIAL);
    assert_eq!(headers["date"], "<date>");
    assert_eq!(headers["retry-after"], "17");
    let url = normalize_url("https://example.com/chat?%6bey=private&model=keep&KEY=other").unwrap();
    assert!(!url.contains("private") && !url.contains("other"));
    let pairs: Vec<_> = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("key".into(), CREDENTIAL.into()),
            ("model".into(), "keep".into()),
            ("KEY".into(), CREDENTIAL.into())
        ]
    );
    assert_eq!(normalize_url(&url).unwrap(), url);
}

#[test]
fn json_metadata_is_removed_but_model_and_usage_are_not() {
    let raw = json!({"id":"abc","created":123,"model":"gpt-4.1-mini","usage":{"prompt_tokens":9},"choices":[{"message":{"content":"hello"}}]});
    let once = Normalizer::default()
        .body(&serde_json::to_vec(&raw).unwrap(), BodyFormat::Json)
        .unwrap();
    let actual: Value = serde_json::from_slice(&once).unwrap();
    assert_eq!(
        actual,
        json!({"id":"<id:1>","created":"<date>","model":"gpt-4.1-mini","usage":{"prompt_tokens":9},"choices":[{"message":{"content":"hello"}}]})
    );
    assert_eq!(
        Normalizer::default().body(&once, BodyFormat::Json).unwrap(),
        once
    );
}

fn corpus_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut corpus = Corpus {
        case_ids: Vec::new(),
        cases: Vec::new(),
        operations: vec![Operation {
            provider: "openai".into(),
            operation: "chat".into(),
            streaming: true,
        }],
    };
    for (id, status, format, body) in [
        (
            "plain",
            200,
            BodyFormat::Json,
            b"{\"choices\":[]}".as_slice(),
        ),
        (
            "stream",
            200,
            BodyFormat::Sse,
            b"data: {\"choices\":[]}\n\ndata: [DONE]\n\n".as_slice(),
        ),
        (
            "error",
            429,
            BodyFormat::Json,
            b"{\"error\":\"rate limited\"}".as_slice(),
        ),
    ] {
        let mut recording = Recording {
            id: id.into(),
            provider: "openai".into(),
            operation: "chat".into(),
            provenance: Provenance::Live,
            request: Request {
                method: "POST".into(),
                url: "https://api.openai.com/v1/chat/completions".into(),
                headers: BTreeMap::from([("authorization".into(), CREDENTIAL.into())]),
                body: json!({"model":"fixture","messages":[]}),
            },
            response: Response {
                status,
                format,
                headers: BTreeMap::new(),
                body: format!("{id}.body"),
            },
        };
        let body = normalize(&mut recording, body).unwrap();
        let file = format!("{id}.json");
        std::fs::write(dir.path().join(&file), json_bytes(&recording).unwrap()).unwrap();
        std::fs::write(dir.path().join(&recording.response.body), body).unwrap();
        corpus.case_ids.push(id.into());
        corpus.cases.push(file);
    }
    std::fs::write(dir.path().join("index.json"), json_bytes(&corpus).unwrap()).unwrap();
    check_corpus(dir.path()).unwrap();
    dir
}

#[test]
fn corpus_rejects_missing_parts_even_if_manifest_matches() {
    for id in ["plain", "stream", "error"] {
        let dir = corpus_fixture();
        let path = dir.path().join("index.json");
        let mut corpus: Corpus = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        corpus.case_ids.retain(|case| case != id);
        corpus.cases.retain(|file| file != &format!("{id}.json"));
        std::fs::write(path, json_bytes(&corpus).unwrap()).unwrap();
        std::fs::remove_file(dir.path().join(format!("{id}.json"))).unwrap();
        std::fs::remove_file(dir.path().join(format!("{id}.body"))).unwrap();
        assert!(
            check_corpus(dir.path())
                .unwrap_err()
                .to_string()
                .contains("incomplete plain/stream/error coverage")
        );
    }
}

#[test]
fn corpus_requires_complete_successful_openai_streams() {
    for (body, valid) in [
        ("data: {\"choices\":[]}\n\n", false),
        (": no response events\n\n", false),
        ("data: [DONE]\n\n", false),
        (
            "data: {\"choices\":[]}\n\ndata: [DONE]\n\ndata: {\"choices\":[]}\n\n",
            false,
        ),
        (
            "data: {\"choices\":[]}\n\ndata: [DONE]\n\ndata: [DONE]\n\n",
            false,
        ),
        ("data: {\"choices\":[]}\n\ndata: [DONE]\n\n", true),
        (
            ": keepalive\r\n\r\ndata: {\"choices\":[]}\r\n\r\ndata: [DONE]\r\n\r\n",
            true,
        ),
    ] {
        let dir = corpus_fixture();
        std::fs::write(dir.path().join("stream.body"), body).unwrap();
        let result = check_corpus(dir.path());
        assert_eq!(result.is_ok(), valid, "body: {body:?}, result: {result:?}");
    }
}

#[test]
fn corpus_rejects_credential_leaks_and_unlisted_cases() {
    for query in [false, true] {
        let dir = corpus_fixture();
        let path = dir.path().join("plain.json");
        let mut recording: Recording =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if query {
            recording.request.url.push_str("?key=private");
        } else {
            recording
                .request
                .headers
                .insert("authorization".into(), "Bearer private".into());
        }
        std::fs::write(path, json_bytes(&recording).unwrap()).unwrap();
        let error = check_corpus(dir.path()).unwrap_err().to_string();
        assert!(error.contains("not normalized") && !error.contains("private"));
    }
    let dir = corpus_fixture();
    std::fs::write(dir.path().join("unlisted.json"), b"{}").unwrap();
    assert!(
        check_corpus(dir.path())
            .unwrap_err()
            .to_string()
            .contains("unlisted corpus file")
    );
    std::fs::remove_file(dir.path().join("unlisted.json")).unwrap();
    let path = dir.path().join("index.json");
    let mut corpus: Corpus = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    corpus.case_ids[0] = "wrong-id".into();
    std::fs::write(path, json_bytes(&corpus).unwrap()).unwrap();
    assert!(
        check_corpus(dir.path())
            .unwrap_err()
            .to_string()
            .contains("declared case-ID set")
    );
}

#[test]
fn corpus_does_not_accept_injected_success_as_provider_evidence() {
    let dir = corpus_fixture();
    let path = dir.path().join("plain.json");
    let mut recording: Recording = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    recording.provenance = Provenance::FaultInjection;
    std::fs::write(path, json_bytes(&recording).unwrap()).unwrap();
    assert!(
        check_corpus(dir.path())
            .unwrap_err()
            .to_string()
            .contains("live provenance")
    );
}

#[test]
fn shared_vectors_and_faults_pin_scalar_counts_and_error_wire_facts() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sdks/ai-contract");
    contract::check(&directory).unwrap();
    let vectors: Value =
        serde_json::from_slice(&std::fs::read(directory.join("estimate-tokens.json")).unwrap())
            .unwrap();
    for case in vectors["cases"].as_array().unwrap() {
        let count = case["text"].as_str().unwrap().chars().count() as u64;
        assert_eq!(case["scalarCount"], count);
        assert_eq!(case["ratio4"], count.div_ceil(4));
        assert_eq!(case["ratio3_5"], (count * 2).div_ceil(7));
    }
    let faults = openai_faults().unwrap();
    assert_eq!(
        faults.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        ["429", "503", "malformed"]
    );
    assert_eq!(faults[0].status, 429);
    assert_eq!(faults[0].headers["retry-after"], "17");
    assert_eq!(
        faults[0].expected,
        json!({"kind":"unavailable","retryAfterSeconds":17})
    );
    assert_eq!(faults[1].status, 503);
    assert_eq!(faults[1].expected, json!({"kind":"unavailable"}));
    assert_eq!(faults[2].status, 200);
    assert_eq!(faults[2].format, BodyFormat::Malformed);
    assert!(serde_json::from_str::<Value>(&faults[2].body).is_err());
    assert_eq!(faults[2].expected, json!({"kind":"invalid-output"}));
}

#[test]
fn contract_rejects_unlisted_assets_and_ids() {
    let directory = tempfile::tempdir().unwrap();
    let recordings = corpus_fixture();
    std::fs::create_dir(directory.path().join("openai")).unwrap();
    for entry in std::fs::read_dir(recordings.path()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(
            entry.path(),
            directory.path().join("openai").join(entry.file_name()),
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("corpus-index.json"),
        br#"{"files":["vectors.json"],"recordingSets":["openai"]}"#,
    )
    .unwrap();
    let vectors_path = directory.path().join("vectors.json");
    let vectors = "{\"caseIds\":[\"astral\"],\"cases\":[{\"id\":\"astral\",\"text\":\"😀\"}]}\n";
    std::fs::write(&vectors_path, vectors).unwrap();
    contract::check(directory.path()).unwrap();
    std::fs::write(directory.path().join("forgotten.json"), b"{}").unwrap();
    assert!(
        contract::check(directory.path())
            .unwrap_err()
            .to_string()
            .contains("unlisted")
    );
    std::fs::remove_file(directory.path().join("forgotten.json")).unwrap();
    std::fs::write(
        vectors_path,
        br#"{"caseIds":["wrong"],"cases":[{"id":"astral"}]}"#,
    )
    .unwrap();
    assert!(
        format!("{:#}", contract::check(directory.path()).unwrap_err())
            .contains("declared case-ID set")
    );
}
