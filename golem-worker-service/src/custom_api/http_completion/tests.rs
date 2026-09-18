use super::*;
use http::{HeaderMap, StatusCode};
use serde_json::Value;
use test_r::test;

fn head(length: Option<u64>, policy: ResponseBodyPolicy) -> ResponseHead {
    ResponseHead {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        content_length: length,
        body_policy: policy,
    }
}

fn corpus_case(id: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    )))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing shared vector {id}"))
        .clone()
}

fn decode_hex(hex: &str) -> Bytes {
    Bytes::from(
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>(),
    )
}

struct Observed {
    body: Vec<u8>,
    terminal: Option<GateTerminal>,
    committed_heads: u64,
    body_polls: u64,
    may_commit_head: bool,
}

fn run_lifecycle(id: &str) -> (Value, Observed) {
    let case = corpus_case(id);
    let events = case["input"]["events"].as_array().unwrap();
    let method_is_head = case["input"]["method"] == "HEAD";
    let head_event = events
        .iter()
        .filter_map(Value::as_str)
        .find(|event| event.starts_with("response-head:"))
        .unwrap();
    let length = head_event
        .split("content-length=")
        .nth(1)
        .map(|value| value.parse().unwrap());
    let policy = if method_is_head {
        ResponseBodyPolicy::Bodyless
    } else {
        ResponseBodyPolicy::Stream
    };
    let mut gate = BodyGate::new(&head(length, policy));
    let mut observed = Observed {
        body: Vec::new(),
        terminal: None,
        committed_heads: 0,
        body_polls: 0,
        may_commit_head: false,
    };
    for event in events.iter().filter_map(Value::as_str) {
        let output = if let Some(hex) = event.strip_prefix("response-chunk:") {
            observed.body_polls += 1;
            gate.push(decode_hex(hex))
        } else {
            match event {
                "response-eof" => {
                    observed.body_polls += 1;
                    gate.body_eof()
                }
                "session-success" => gate.session_success(),
                "session-failure" => gate.session_failure(),
                "executor-loss" => gate.producer_failure(),
                "commit-head" => {
                    observed.committed_heads += u64::from(gate.may_commit_head());
                    GateOutput::default()
                }
                _ => GateOutput::default(),
            }
        };
        if let Some(bytes) = output.bytes {
            observed.body.extend_from_slice(&bytes);
        }
        if output.terminal.is_some() {
            observed.terminal = output.terminal;
        }
    }
    observed.may_commit_head = gate.may_commit_head();
    (case, observed)
}

fn assert_gate_expectations(case: &Value, observed: &Observed) {
    let expect = &case["expect"];
    if let Some(hex) = expect.get("body_hex").and_then(Value::as_str) {
        assert_eq!(observed.body, decode_hex(hex));
    }
    if let Some(maximum) = expect
        .get("maximum_public_body_bytes")
        .and_then(Value::as_u64)
    {
        assert!(observed.body.len() as u64 <= maximum);
    }
    if let Some(expected) = expect.get("public_abort").and_then(Value::as_bool) {
        assert_eq!(
            matches!(observed.terminal, Some(GateTerminal::Abort(_))),
            expected
        );
    }
    if let Some(expected) = expect.get("successful_eof").and_then(Value::as_bool) {
        assert_eq!(observed.terminal == Some(GateTerminal::Complete), expected);
    }
    if let Some(expected) = expect.get("committed_heads").and_then(Value::as_u64) {
        assert_eq!(observed.committed_heads, expected);
    }
    if let Some(expected) = expect.get("body_polls").and_then(Value::as_u64) {
        assert_eq!(observed.body_polls, expected);
    }
}

#[test]
fn shared_lifecycle_vectors_drive_the_gate() {
    for id in [
        "lifecycle-body-eof-session-fails",
        "lifecycle-fixed-length-session-fails",
        "lifecycle-fixed-length-session-succeeds",
        "lifecycle-short-promised-response",
        "lifecycle-overlong-response",
        "lifecycle-executor-loss-after-head",
    ] {
        let (case, observed) = run_lifecycle(id);
        assert_gate_expectations(&case, &observed);
        if matches!(observed.terminal, Some(GateTerminal::Abort(_))) {
            assert!(!observed.may_commit_head, "{id} allowed commit after abort");
        }
    }
}

#[test]
fn shared_bodyless_disposal_vectors_drive_the_gate() {
    for id in [
        "lifecycle-head-disposal-session-fails",
        "lifecycle-head-disposal-session-succeeds",
    ] {
        let (case, observed) = run_lifecycle(id);
        assert_gate_expectations(&case, &observed);
        let expected_commits = case["expect"]["committed_application_heads"]
            .as_u64()
            .unwrap();
        assert_eq!(u64::from(observed.may_commit_head), expected_commits);
    }
}

#[test]
fn fixed_length_only_retains_the_promised_final_byte() {
    let mut gate = BodyGate::new(&head(Some(4), ResponseBodyPolicy::Stream));
    assert_eq!(
        gate.push(Bytes::from_static(b"ab")).bytes.unwrap(),
        b"ab"[..]
    );
    assert_eq!(
        gate.push(Bytes::from_static(b"cd")).bytes.unwrap(),
        b"c"[..]
    );
    assert_eq!(gate.body_eof(), GateOutput::default());
    let done = gate.session_success();
    assert_eq!(done.bytes.unwrap(), b"d"[..]);
    assert_eq!(done.terminal, Some(GateTerminal::Complete));
}

#[test]
fn output_debug_is_opaque() {
    let output = GateOutput {
        bytes: Some(Bytes::from_static(b"secret-response")),
        terminal: None,
    };
    assert_eq!(
        format!("{output:?}"),
        "GateOutput { bytes_len: Some(15), terminal: None }"
    );
}
