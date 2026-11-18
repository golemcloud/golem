// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (https://license.golem.cloud/LICENSE).

use golem_schema_derive::PoemSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind")]
enum ScalarPayload {
    Value(String),
}

#[derive(Serialize, Deserialize, PoemSchema)]
#[serde(tag = "inner")]
enum Composed {
    Value { value: String },
}

#[derive(Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind")]
enum ComposedPayload {
    Value(Composed),
}

#[derive(Serialize, Deserialize, PoemSchema)]
struct ObjectPayload {
    value: String,
}

#[derive(Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind")]
enum NullablePayload {
    Value(Option<ObjectPayload>),
}

#[derive(Serialize, Deserialize, PoemSchema)]
#[serde(tag = "kind")]
struct TaggedPayload {
    value: String,
}

#[derive(PoemSchema)]
#[serde(content = "value")]
struct ContentPayload {
    value: String,
}

#[derive(PoemSchema)]
#[serde(tag = "kind")]
struct TaggedNewtype(String);

fn main() {}
