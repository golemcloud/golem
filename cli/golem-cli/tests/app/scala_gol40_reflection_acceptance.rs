// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::{TestContext, cmd, flag};
use serde_json::{Value, json};
use std::collections::HashMap;
use test_r::{test, timeout};

const FIXTURE: &str = "scala-gol40-reflection-acceptance";

#[test]
#[timeout("20 minutes")]
async fn scala_gol40_generated_and_reflected_clients_match_live() {
    let mut ctx = TestContext::new();
    fs_extra::dir::copy(
        ctx.test_data_path_join(FIXTURE),
        ctx.cwd_path(),
        &fs_extra::dir::CopyOptions::new(),
    )
    .unwrap();
    ctx.cd(FIXTURE);
    ctx.start_server().await;

    let built = ctx.cli([flag::YES, cmd::BUILD, flag::FORCE_BUILD]).await;
    assert!(built.success_or_dump());
    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());

    let invoked = ctx
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            "ScalaGol40ReflectionAcceptance(\"cli-acceptance\")",
            "observe",
            flag::FORMAT,
            "json",
            "--no-stream",
        ])
        .await;
    assert!(invoked.success_or_dump());

    let result = invoked
        .stdout_json::<Value>()
        .into_iter()
        .find(|event| event["$type"] == "agent.invoke")
        .expect("missing reflection observation result");
    let names = result["resultJson"]["graph"]["defs"][0]["body"]["value"]["fields"]
        .as_array()
        .expect("reflection observation schema is a record");
    let values = result["resultJson"]["value"]["value"]["fields"]
        .as_array()
        .expect("reflection observation value is a record");
    let observation = names
        .iter()
        .zip(values)
        .map(|(field, value)| (field["name"].as_str().unwrap(), &value["value"]))
        .collect::<HashMap<_, _>>();

    assert_eq!(observation["namedClientBound"], true);
    assert_eq!(observation["nestedMatches"], true);
    assert_eq!(observation["typedErrorMatches"], true);
    assert_eq!(observation["principalMatches"], true);
    assert_eq!(observation["streamMatches"], true);
    assert_eq!(
        observation["generatedNested"],
        "Gol40ReflectionOutput(nested/inspect,shared-prefix:asymmetric,89,golem-user)"
    );
    assert_eq!(
        observation["reflectedNested"],
        "{\n  \"path\": \"nested/inspect\",\n  \"label\": \"shared-prefix:asymmetric\",\n  \"weighted\": \"89\",\n  \"principal\": \"golem-user\"\n}"
    );
    assert_eq!(observation["generatedPrincipal"], "golem-user");
    assert_eq!(observation["reflectedPrincipal"], "\"golem-user\"");
    let expected_stream_bytes = json!({
        "elements": [
            {"kind": "s32", "value": 0},
            {"kind": "s32", "value": 2},
            {"kind": "s32", "value": 5},
            {"kind": "s32", "value": 9},
            {"kind": "s32", "value": -1}
        ]
    });
    assert_eq!(observation["generatedStreamBytes"], &expected_stream_bytes);
    assert_eq!(observation["reflectedStreamBytes"], &expected_stream_bytes);
    assert_eq!(
        observation["generatedStreamResult"],
        "streamed:shared-stream"
    );
    assert_eq!(
        observation["reflectedStreamResult"],
        "\"streamed:shared-stream\""
    );
    assert_eq!(result["$type"], "agent.invoke");
}
