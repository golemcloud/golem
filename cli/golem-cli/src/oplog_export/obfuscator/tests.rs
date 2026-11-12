// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::canonical::{encode, encode_str};
use super::value::{Region, Walker};
use super::*;
use crate::oplog_export::model::{CaseNode, ListNode, OmittedReason, ReportEntry, ReportNode};
use base64::Engine;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::host_functions;
use golem_common::model::oplog::public_oplog_entry::*;
use golem_common::model::oplog::types::{
    FileSystemError, SerializableFsErrorCode, SerializableHostFailureKind,
    SerializableHttpErrorCode, SerializableHttpMethod, SerializableInvokeResult,
    SerializableP3FileSystemError, SerializableP3FsErrorCode, SerializableP3HttpClientSendResult,
    SerializableResponseHeaders, SerializableRpcError,
};
use golem_common::model::oplog::{
    AgentError, HostPayloadPair, HostRequest, HostRequestGolemApiGetAgents,
    HostRequestGolemRpcInvoke, HostRequestRandomBytes, HostResponse, HostResponseEntityInvocation,
    HostResponseFileSystemStat, HostResponseGolemAgentGetConfigValue, HostResponseGolemRpcActivate,
    HostResponseGolemRpcCreate, HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcInvokeGet,
    HostResponseKVGet, HostResponseP3FileSystemStat, HostResponseP3HttpClientSendResult,
    OmittedOplogEntry, PublicDurableFunctionType, PublicOplogEntry, PublicOplogEntryAttribution,
    PublicOplogEntryWithIndex,
};
use golem_common::model::{
    AgentFingerprint, AgentId, Empty, IdempotencyKey, OplogIndex, Timestamp,
};
use golem_common::schema::{
    BinaryValuePayload, FromSchema, IntoSchema, IntoTypedSchemaValue, NamedFieldType, QuantitySpec,
    QuantityValue, SchemaGraph, SchemaType, SchemaValue, TextValuePayload, TypedSchemaValue,
    VariantValuePayload,
};
use proptest::prelude::*;
use std::collections::HashMap;
use test_r::test;
use uuid::Uuid;

const KEY: [u8; 32] = {
    let mut key = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        key[i] = i as u8;
        i += 1;
    }
    key
};

fn key() -> TokenKey {
    TokenKey::from_bytes(KEY)
}

fn json(node: &impl serde::Serialize) -> String {
    serde_json::to_string(node).unwrap()
}

fn component_id() -> ComponentId {
    ComponentId(Uuid::parse_str("13a5c8d4-f05e-4e23-b982-f4d413e181cb").unwrap())
}

fn agent_id(name: &str) -> AgentId {
    AgentId {
        component_id: component_id(),
        agent_id: name.to_string(),
    }
}

/// Renders a schema-less value in the customer region.
fn render(keep: KeepFlags, value: &SchemaValue) -> ReportNode {
    let key = key();
    let ctx = ObfuscationContext::new(&key, keep);
    Walker::new(&ctx).schemaless(value, Region::Customer)
}

/// Renders a typed value in the customer region.
fn render_typed(keep: KeepFlags, typed: &TypedSchemaValue) -> ReportNode {
    let key = key();
    let ctx = ObfuscationContext::new(&key, keep);
    Walker::new(&ctx).customer(typed)
}

fn all_keep() -> [KeepFlags; 8] {
    let mut out = [KeepFlags::default(); 8];
    for (i, flags) in out.iter_mut().enumerate() {
        *flags = KeepFlags {
            scalars: i & 1 != 0,
            shape: i & 2 != 0,
            error_text: i & 4 != 0,
        };
    }
    out
}

fn with_entry(index: u64, entry: PublicOplogEntry) -> PublicOplogEntryWithIndex {
    let entry = PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(index),
        attribution: PublicOplogEntryAttribution::agent(),
        entry,
    };
    // The CLI receives entries as JSON, so every fixture takes the same path.
    let text = serde_json::to_string(&entry).unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn start(index: u64, function: &str, request: TypedSchemaValue) -> PublicOplogEntryWithIndex {
    with_entry(
        index,
        PublicOplogEntry::Start(StartParams {
            timestamp: Timestamp::from(0),
            parent_start_index: None,
            function_name: function.to_string(),
            invocation_id: None,
            observational_owner: None,
            request: Some(request),
            durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
            span_started: None,
            denial: None,
        }),
    )
}

fn end(index: u64, response: TypedSchemaValue) -> PublicOplogEntryWithIndex {
    with_entry(
        index,
        PublicOplogEntry::End(EndParams {
            timestamp: Timestamp::from(0),
            start_index: OplogIndex::from_u64(index - 1),
            response: Some(response),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }),
    )
}

fn request(request: HostRequest) -> TypedSchemaValue {
    request.into_typed_schema_value().unwrap()
}

fn response(response: HostResponse) -> TypedSchemaValue {
    response.into_typed_schema_value().unwrap()
}

fn rendered(
    keep: KeepFlags,
    entry: &PublicOplogEntryWithIndex,
    start_function: Option<&str>,
) -> RenderedEntry {
    let key = key();
    render_entry(&ObfuscationContext::new(&key, keep), entry, start_function)
}

fn fqfn<P: HostPayloadPair>() -> &'static str {
    P::FQFN
}

/// The body field `name` of a rendered entry.
fn field<'a>(entry: &'a ReportEntry, name: &str) -> &'a ReportNode {
    let Some(ReportNode::Record(body)) = &entry.body else {
        panic!("body is not a record: {entry:?}");
    };
    &body
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no field {name}"))
        .value
}

fn record_field<'a>(node: &'a ReportNode, name: &str) -> &'a ReportNode {
    let ReportNode::Record(record) = node else {
        panic!("not a record: {node:?}");
    };
    &record
        .fields
        .iter()
        .find(|field| {
            super::value::normalized(field.name.as_deref().unwrap_or_default())
                == super::value::normalized(name)
        })
        .unwrap_or_else(|| panic!("no field {name} in {node:?}"))
        .value
}

fn as_case(node: &ReportNode) -> &CaseNode {
    match node {
        ReportNode::Case(case) => case,
        other => panic!("not a case: {other:?}"),
    }
}

/// Replaces every token with a placeholder, so two renders compare by structure.
fn without_tokens(text: &str) -> String {
    let re = regex::Regex::new(r#""[a-z:]+:[0-9a-f]{32}""#).unwrap();
    re.replace_all(text, "\"T\"").to_string()
}

fn assert_no_marker(text: &str, marker: &str) {
    assert!(!text.contains(marker), "marker {marker} in {text}");
    for engine in [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ] {
        for shift in 0..3 {
            let padded = format!("{}{}", "x".repeat(shift), marker);
            let encoded = engine.encode(padded.as_bytes());
            let core = &encoded[(shift * 4).div_ceil(3)..encoded.len().saturating_sub(4)];
            if core.len() >= 6 {
                assert!(!text.contains(core), "base64 of {marker} in {text}");
            }
        }
    }
    let decimal = marker
        .bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",");
    assert!(
        !text.contains(&decimal),
        "decimal bytes of {marker} in {text}"
    );
}

// -------------------------------------------------------------------------------------------
// Key and context
// -------------------------------------------------------------------------------------------

#[test]
fn process_key_is_drawn_once() {
    let first = TokenKey::process();
    let others: Vec<usize> = std::thread::scope(|scope| {
        (0..8)
            .map(|_| scope.spawn(|| TokenKey::process() as *const TokenKey as usize))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    for other in others {
        assert_eq!(other, first as *const TokenKey as usize);
    }
    assert!(std::ptr::eq(first, TokenKey::process()));
    assert_eq!(TokenKey::process_key_draws(), 1);
}

#[test]
fn two_contexts_in_one_process_share_tokens() {
    let a = ObfuscationContext::new(TokenKey::process(), KeepFlags::default());
    let b = ObfuscationContext::new(
        TokenKey::process(),
        KeepFlags {
            scalars: true,
            shape: false,
            error_text: true,
        },
    );
    let value = SchemaValue::List {
        elements: vec![SchemaValue::String("x".into())],
    };
    let render =
        |ctx: &ObfuscationContext| json(&Walker::new(ctx).schemaless(&value, Region::Customer));
    assert_eq!(render(&a), render(&b));
    assert_eq!(
        agent_reference(&a, &agent_id("a")),
        agent_reference(&b, &agent_id("a"))
    );
}

#[test]
fn different_keys_give_different_tokens() {
    let k1 = TokenKey::from_bytes([1; 32]);
    let k2 = TokenKey::from_bytes([2; 32]);
    let k1_again = TokenKey::from_bytes([1; 32]);
    let token = |key: &TokenKey| {
        ObfuscationContext::new(key, KeepFlags::default()).token("str", &encode_str("value"))
    };
    assert_ne!(token(&k1), token(&k2));
    assert_eq!(token(&k1), token(&k1_again));
}

#[test]
fn key_never_in_output() {
    let bytes: [u8; 32] = *b"KEYBYTES-0123456789-abcdefghijkl";
    let key = TokenKey::from_bytes(bytes);
    let ctx = ObfuscationContext::new(&key, KeepFlags::default());
    let mut outputs = vec![format!("{key:?}"), format!("{ctx:?}")];
    for entry in every_entry_kind() {
        outputs.push(json(&render_entry(&ctx, &entry, None).entry));
    }
    let hex_lower: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hex_upper = hex_lower.to_uppercase();
    for output in outputs {
        assert_no_marker(&output, std::str::from_utf8(&bytes).unwrap());
        assert!(!output.contains(&hex_lower[..16]));
        assert!(!output.contains(&hex_upper[..16]));
    }
}

// -------------------------------------------------------------------------------------------
// Values and tokens
// -------------------------------------------------------------------------------------------

#[test]
fn token_golden_vector() {
    // Expected values computed independently with Python's hmac module.
    let key = key();
    let ctx = ObfuscationContext::new(&key, KeepFlags::default());
    assert_eq!(
        ctx.token("str", &encode(&SchemaValue::String("abc".into())))
            .0,
        "str:1928eb0d4d07a51ff0d6d87a39128264"
    );
    assert_eq!(
        ctx.token(
            "list",
            &encode(&SchemaValue::List {
                elements: vec![
                    SchemaValue::String("a".into()),
                    SchemaValue::String("b".into())
                ]
            })
        )
        .0,
        "list:391e2f12efe648e26446b31021a9fd06"
    );
    assert_eq!(
        ctx.token(
            "map",
            &encode(&SchemaValue::Map {
                entries: vec![(
                    SchemaValue::String("k".into()),
                    SchemaValue::String("v".into())
                )]
            })
        )
        .0,
        "map:863a964ddc59021402cab6036bbb088c"
    );
    assert_eq!(
        agent_reference(
            &ctx,
            &agent_id("counter(\"a\")[00000000-0000-0000-0000-000000000001]")
        )
        .token
        .0,
        "agent:b05170b8b32930ed338456095adb0702"
    );
}

#[test]
fn str_kinds_share_one_token() {
    let keep = KeepFlags::default();
    let string = json(&render(keep, &SchemaValue::String("text".into())));
    for other in [
        SchemaValue::Text(TextValuePayload {
            text: "text".into(),
            language: Some("en".into()),
        }),
        SchemaValue::Url { url: "text".into() },
        SchemaValue::Path {
            path: "text".into(),
        },
    ] {
        assert_eq!(json(&render(keep, &other)), string);
    }
    assert_eq!(
        json(&render(keep, &SchemaValue::Char('x'))),
        json(&render(keep, &SchemaValue::String("x".into())))
    );
    let uuid = Uuid::from_u128(7);
    assert_eq!(
        json(&render(keep, &SchemaValue::Uuid(uuid))),
        json(&render(
            keep,
            &SchemaValue::String(uuid.hyphenated().to_string())
        ))
    );
}

#[test]
fn integer_kinds_share_one_token() {
    let keep = KeepFlags::default();
    assert_eq!(
        json(&render(keep, &SchemaValue::S32(5))),
        json(&render(keep, &SchemaValue::U64(5)))
    );
    assert_ne!(
        json(&render(keep, &SchemaValue::S32(5))),
        json(&render(keep, &SchemaValue::F64(5.0)))
    );
}

#[test]
fn length_prefixes_separate_fields() {
    let keep = KeepFlags::default();
    let tuple = |a: &str, b: &str| SchemaValue::List {
        elements: vec![SchemaValue::Tuple {
            elements: vec![SchemaValue::String(a.into()), SchemaValue::String(b.into())],
        }],
    };
    assert_ne!(
        json(&render(keep, &tuple("ab", "c"))),
        json(&render(keep, &tuple("a", "bc")))
    );
    let map = |a: &str, b: &str| SchemaValue::Map {
        entries: vec![(SchemaValue::String(a.into()), SchemaValue::String(b.into()))],
    };
    assert_ne!(
        json(&render(keep, &map("ab", "c"))),
        json(&render(keep, &map("a", "bc")))
    );
}

#[test]
fn map_order_independent() {
    let pairs: Vec<(String, u32)> = (0..20).map(|i| (format!("key-{i}"), i)).collect();
    let forward = SchemaValue::Map {
        entries: pairs
            .iter()
            .map(|(k, v)| (SchemaValue::String(k.clone()), SchemaValue::U32(*v)))
            .collect(),
    };
    let backward = SchemaValue::Map {
        entries: pairs
            .iter()
            .rev()
            .map(|(k, v)| (SchemaValue::String(k.clone()), SchemaValue::U32(*v)))
            .collect(),
    };
    let hash_a: HashMap<String, u32> = pairs.iter().cloned().collect();
    let hash_b: HashMap<String, u32> = pairs.iter().rev().cloned().collect();
    for keep in all_keep() {
        let expected = json(&render(keep, &forward));
        assert_eq!(json(&render(keep, &backward)), expected);
        assert_eq!(json(&render(keep, &hash_a.to_value())), expected);
        assert_eq!(json(&render(keep, &hash_b.to_value())), expected);
        let in_list = |map: &SchemaValue| SchemaValue::List {
            elements: vec![map.clone()],
        };
        assert_eq!(
            json(&render(keep, &in_list(&forward))),
            json(&render(keep, &in_list(&backward)))
        );
    }
    let list = |a: &str, b: &str| SchemaValue::List {
        elements: vec![SchemaValue::String(a.into()), SchemaValue::String(b.into())],
    };
    assert_ne!(
        json(&render(KeepFlags::default(), &list("a", "b"))),
        json(&render(KeepFlags::default(), &list("b", "a")))
    );
}

#[test]
fn empty_lists_share_one_token() {
    let keep = KeepFlags::default();
    let empty = SchemaValue::List { elements: vec![] };
    let bytes = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::list(SchemaType::u8())),
        empty.clone(),
    );
    let strings = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::list(SchemaType::string())),
        empty.clone(),
    );
    let expected = json(&render(keep, &empty));
    assert_eq!(json(&render_typed(keep, &bytes)), expected);
    assert_eq!(json(&render_typed(keep, &strings)), expected);
}

#[test]
fn byte_list_never_shows_elements() {
    let keep = KeepFlags {
        scalars: true,
        shape: true,
        error_text: true,
    };
    let value = SchemaValue::List {
        elements: b"BYTES".iter().map(|b| SchemaValue::U8(*b)).collect(),
    };
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::list(SchemaType::u8())),
        value.clone(),
    );
    let fixed = SchemaValue::FixedList {
        elements: b"BYTES".iter().map(|b| SchemaValue::U8(*b)).collect(),
    };
    let typed_fixed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::fixed_list(SchemaType::u8(), 5)),
        fixed.clone(),
    );
    for node in [
        render(keep, &value),
        render_typed(keep, &typed),
        render(keep, &fixed),
        render_typed(keep, &typed_fixed),
    ] {
        let ReportNode::List(ListNode {
            length, elements, ..
        }) = &node
        else {
            panic!("not a list: {node:?}");
        };
        assert_eq!(*length, Some(5));
        assert!(elements.is_none());
    }
    assert_eq!(
        json(&render(keep, &value)),
        json(&render_typed(keep, &typed))
    );

    // The same bytes under a declared element type that is not `u8`.
    let mistyped = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::list(SchemaType::string())),
        value.clone(),
    );
    let node = render_typed(keep, &mistyped);
    let ReportNode::List(ListNode { elements, .. }) = &node else {
        panic!("not a list: {node:?}");
    };
    assert!(elements.is_none());
    assert_eq!(json(&node), json(&render(keep, &value)));
    assert_no_marker(&json(&node), "BYTES");
}

fn shaped(case: u32, flag: bool, present: bool, list_len: usize, body_len: usize) -> SchemaValue {
    SchemaValue::Record {
        fields: vec![
            SchemaValue::Enum { case },
            SchemaValue::Flags { bits: vec![flag] },
            SchemaValue::Option {
                inner: present.then(|| Box::new(SchemaValue::U8(1))),
            },
            SchemaValue::List {
                elements: vec![SchemaValue::String("e".into()); list_len],
            },
            SchemaValue::Binary(BinaryValuePayload {
                bytes: vec![0; body_len],
                mime_type: None,
            }),
        ],
    }
}

#[test]
fn shape_hidden_by_default() {
    let a = shaped(0, false, false, 1, 3);
    let b = shaped(1, true, true, 2, 4);
    let keep = KeepFlags::default();
    assert_eq!(
        without_tokens(&json(&render(keep, &a))),
        without_tokens(&json(&render(keep, &b)))
    );
    let shape = KeepFlags {
        shape: true,
        ..KeepFlags::default()
    };
    let shown_a = json(&render(shape, &a));
    assert_ne!(
        without_tokens(&shown_a),
        without_tokens(&json(&render(shape, &b)))
    );
    assert!(leaves_are_tokens(&serde_json::from_str(&shown_a).unwrap()));
}

/// `true` when no `scalar`, `str` or `quantity` node shows its value.
fn leaves_are_tokens(node: &serde_json::Value) -> bool {
    match node {
        serde_json::Value::Object(map) => {
            let leaf = matches!(
                map.get("kind").and_then(|kind| kind.as_str()),
                Some("scalar" | "str" | "quantity")
            );
            !(leaf && (map.contains_key("value") || map.contains_key("mantissa")))
                && map.values().all(leaves_are_tokens)
        }
        serde_json::Value::Array(elements) => elements.iter().all(leaves_are_tokens),
        _ => true,
    }
}

#[test]
fn keep_scalars_only_where_nodes_expand() {
    let scalars = KeepFlags {
        scalars: true,
        ..KeepFlags::default()
    };
    let in_record = render(
        scalars,
        &SchemaValue::Record {
            fields: vec![SchemaValue::U32(4711)],
        },
    );
    assert!(json(&in_record).contains("4711"));
    for collapsed in [
        SchemaValue::List {
            elements: vec![SchemaValue::U32(4711)],
        },
        SchemaValue::FixedList {
            elements: vec![SchemaValue::U32(4711)],
        },
        SchemaValue::Variant(VariantValuePayload {
            case: 0,
            payload: Some(Box::new(SchemaValue::U32(4711))),
        }),
    ] {
        assert!(!json(&render(scalars, &collapsed)).contains("4711"));
    }
    let fixed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::fixed_list(SchemaType::u32(), 1)),
        SchemaValue::FixedList {
            elements: vec![SchemaValue::U32(4711)],
        },
    );
    assert!(!json(&render_typed(scalars, &fixed)).contains("4711"));
    let shape_and_scalars = KeepFlags {
        scalars: true,
        shape: true,
        ..KeepFlags::default()
    };
    assert!(json(&render_typed(shape_and_scalars, &fixed)).contains("4711"));
}

#[test]
fn quantity_units() {
    let scalars = KeepFlags {
        scalars: true,
        ..KeepFlags::default()
    };
    let value = SchemaValue::Quantity(QuantityValue {
        mantissa: 15,
        scale: 1,
        unit: "kg".into(),
    });
    let typed = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::quantity(QuantitySpec {
            base_unit: "kg".into(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        })),
        value,
    );
    let ReportNode::Quantity(quantity) = render_typed(scalars, &typed) else {
        panic!()
    };
    assert_eq!(
        (quantity.mantissa, quantity.scale, quantity.unit.as_deref()),
        (Some(15), Some(1), Some("kg"))
    );
    let schemaless = render(
        scalars,
        &SchemaValue::Quantity(QuantityValue {
            mantissa: 15,
            scale: 1,
            unit: "MARKER".into(),
        }),
    );
    let ReportNode::Quantity(quantity) = &schemaless else {
        panic!()
    };
    assert!(quantity.unit.is_none() && quantity.unit_token.is_some());
    assert_no_marker(&json(&schemaless), "MARKER");

    // A typed value whose unit its type does not declare: the unit is free text.
    let undeclared = TypedSchemaValue::new(
        typed.graph().clone(),
        SchemaValue::Quantity(QuantityValue {
            mantissa: 15,
            scale: 1,
            unit: "MARKER".into(),
        }),
    );
    let undeclared = render_typed(scalars, &undeclared);
    let ReportNode::Quantity(quantity) = &undeclared else {
        panic!()
    };
    assert!(quantity.unit.is_none() && quantity.unit_token.is_some());
    assert_no_marker(&json(&undeclared), "MARKER");
}

/// Replaces every customer text and byte payload with a unique marker.
fn plant_markers(value: &mut SchemaValue, counter: &mut usize) {
    let mut next = || {
        *counter += 1;
        format!("MRK{counter}Q")
    };
    match value {
        SchemaValue::String(s) => *s = next(),
        SchemaValue::Text(text) => text.text = next(),
        SchemaValue::Path { path } => *path = next(),
        SchemaValue::Url { url } => *url = next(),
        SchemaValue::Binary(binary) => binary.bytes = next().into_bytes(),
        SchemaValue::Quantity(quantity) => quantity.unit = next(),
        SchemaValue::Union(union) => plant_markers(&mut union.body, counter),
        SchemaValue::Record { fields } => fields.iter_mut().for_each(|v| plant_markers(v, counter)),
        SchemaValue::Tuple { elements }
        | SchemaValue::List { elements }
        | SchemaValue::FixedList { elements } => {
            elements.iter_mut().for_each(|v| plant_markers(v, counter))
        }
        SchemaValue::Map { entries } => entries.iter_mut().for_each(|(k, v)| {
            plant_markers(k, counter);
            plant_markers(v, counter)
        }),
        SchemaValue::Variant(payload) => {
            if let Some(inner) = payload.payload.as_deref_mut() {
                plant_markers(inner, counter)
            }
        }
        SchemaValue::Option { inner } => {
            if let Some(inner) = inner.as_deref_mut() {
                plant_markers(inner, counter)
            }
        }
        SchemaValue::Result(result) => {
            let (golem_common::schema::ResultValuePayload::Ok { value }
            | golem_common::schema::ResultValuePayload::Err { value }) = result;
            if let Some(inner) = value.as_deref_mut() {
                plant_markers(inner, counter)
            }
        }
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn no_marker_in_schema_less_values(
        value in golem_common::schema::proptest_strategies::schema_value_strategy()
    ) {
        let mut value = value;
        let mut counter = 0;
        plant_markers(&mut value, &mut counter);
        for keep in all_keep() {
            let output = json(&render(keep, &value));
            prop_assert!(!output.contains("MRK"), "marker in {}", output);
        }
    }

    #[test]
    fn malformed_pairs_never_panic_or_leak(
        typed in golem_common::schema::proptest_strategies::typed_schema_value_strategy()
    ) {
        let mut value = typed.value().clone();
        let mut counter = 0;
        plant_markers(&mut value, &mut counter);
        let typed = TypedSchemaValue::new(typed.graph().clone(), value);
        for keep in all_keep() {
            let output = json(&render_typed(keep, &typed));
            prop_assert!(!output.contains("MRK"), "marker in {}", output);
        }
    }
}

static NEXT_MARKER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fresh upper-case marker. Type-side names use lower case, so they never hold one.
fn next_marker() -> String {
    let n = NEXT_MARKER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("MRK{n}Q")
}

type Pair = (SchemaType, SchemaValue);

fn leaf_pair() -> impl Strategy<Value = Pair> {
    use golem_common::schema::*;
    prop_oneof![
        any::<bool>().prop_map(|b| (SchemaType::bool(), SchemaValue::Bool(b))),
        any::<i32>().prop_map(|n| (SchemaType::s32(), SchemaValue::S32(n))),
        any::<u64>().prop_map(|n| (SchemaType::u64(), SchemaValue::U64(n))),
        any::<f64>().prop_map(|n| (SchemaType::f64(), SchemaValue::F64(n))),
        any::<u8>().prop_map(|n| (SchemaType::u8(), SchemaValue::U8(n))),
        prop::char::range('a', 'z').prop_map(|c| (SchemaType::char(), SchemaValue::Char(c))),
        Just(()).prop_map(|_| (SchemaType::string(), SchemaValue::String(next_marker()))),
        Just(()).prop_map(|_| (
            SchemaType::text(TextRestrictions::default()),
            SchemaValue::Text(TextValuePayload {
                text: next_marker(),
                language: Some("en".into()),
            }),
        )),
        Just(()).prop_map(|_| (
            SchemaType::binary(BinaryRestrictions::default()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes: next_marker().into_bytes(),
                mime_type: None,
            }),
        )),
        Just(()).prop_map(|_| (
            SchemaType::list(SchemaType::u8()),
            SchemaValue::List {
                elements: next_marker().bytes().map(SchemaValue::U8).collect(),
            },
        )),
        Just(()).prop_map(|_| (
            SchemaType::path(PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::Any,
                allowed_mime_types: None,
                allowed_extensions: None,
            }),
            SchemaValue::Path {
                path: format!("dir/{}", next_marker()),
            },
        )),
        Just(()).prop_map(|_| (
            SchemaType::url(UrlRestrictions::default()),
            SchemaValue::Url {
                url: format!("https://h.example/{}", next_marker()),
            },
        )),
        any::<u128>().prop_map(|n| (SchemaType::uuid(), SchemaValue::Uuid(Uuid::from_u128(n)))),
        (0i64..4_000_000_000).prop_map(|s| (
            SchemaType::datetime(),
            SchemaValue::Datetime {
                value: chrono::DateTime::from_timestamp(s, 0).unwrap(),
            },
        )),
        any::<i64>().prop_map(|n| (
            SchemaType::duration(),
            SchemaValue::Duration(golem_common::schema::schema_value::DurationValuePayload {
                nanoseconds: n,
            }),
        )),
        (any::<i64>(), -6i32..6).prop_map(|(mantissa, scale)| (
            SchemaType::quantity(QuantitySpec {
                base_unit: "byte".into(),
                allowed_suffixes: Vec::new(),
                min: None,
                max: None,
            }),
            SchemaValue::Quantity(QuantityValue {
                mantissa,
                scale,
                unit: "byte".into(),
            }),
        )),
        Just(()).prop_map(|_| (
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "tagged".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix { prefix: "p".into() },
                    metadata: Default::default(),
                }],
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "tagged".into(),
                body: Box::new(SchemaValue::String(format!("p{}", next_marker()))),
            }),
        )),
    ]
}

fn named_field(i: usize, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: format!("f{i}"),
        body,
        metadata: Default::default(),
    }
}

/// A schema type of every value kind with nested containers, and a value that conforms to it.
/// Customer data holds a unique upper-case marker in every text, byte and map key leaf.
fn matching_pair() -> impl Strategy<Value = Pair> {
    use golem_common::schema::*;
    leaf_pair().prop_recursive(4, 48, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(|pairs| {
                let (types, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                (
                    SchemaType::record(
                        types
                            .into_iter()
                            .enumerate()
                            .map(|(i, t)| named_field(i, t))
                            .collect(),
                    ),
                    SchemaValue::Record { fields: values },
                )
            }),
            prop::collection::vec(inner.clone(), 1..4).prop_map(|pairs| {
                let (types, values): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                (
                    SchemaType::tuple(types),
                    SchemaValue::Tuple { elements: values },
                )
            }),
            (
                prop::collection::vec(inner.clone(), 1..4),
                any::<prop::sample::Index>()
            )
                .prop_map(|(pairs, pick)| {
                    let case = pick.index(pairs.len());
                    let value = pairs[case].1.clone();
                    let cases = pairs
                        .into_iter()
                        .enumerate()
                        .map(|(i, (t, _))| VariantCaseType {
                            name: format!("c{i}"),
                            payload: Some(t),
                            metadata: Default::default(),
                        })
                        .collect();
                    (
                        SchemaType::variant(cases),
                        SchemaValue::Variant(VariantValuePayload {
                            case: case as u32,
                            payload: Some(Box::new(value)),
                        }),
                    )
                }),
            (1usize..4, any::<prop::sample::Index>()).prop_map(|(n, pick)| (
                SchemaType::Enum {
                    cases: (0..n).map(|i| format!("e{i}")).collect(),
                    metadata: Default::default(),
                },
                SchemaValue::Enum {
                    case: pick.index(n) as u32,
                },
            )),
            prop::collection::vec(any::<bool>(), 1..4).prop_map(|bits| (
                SchemaType::flags((0..bits.len()).map(|i| format!("g{i}")).collect()),
                SchemaValue::Flags { bits },
            )),
            (inner.clone(), 0usize..3).prop_map(|((t, v), n)| (
                SchemaType::list(t),
                SchemaValue::List {
                    elements: vec![v; n],
                },
            )),
            (inner.clone(), 1u32..3).prop_map(|((t, v), n)| (
                SchemaType::fixed_list(t, n),
                SchemaValue::FixedList {
                    elements: vec![v; n as usize],
                },
            )),
            (inner.clone(), 0usize..3).prop_map(|((t, v), n)| (
                SchemaType::map(SchemaType::string(), t),
                SchemaValue::Map {
                    entries: (0..n)
                        .map(|_| (SchemaValue::String(next_marker()), v.clone()))
                        .collect(),
                },
            )),
            (inner.clone(), any::<bool>()).prop_map(|((t, v), present)| (
                SchemaType::option(t),
                SchemaValue::Option {
                    inner: present.then(|| Box::new(v)),
                },
            )),
            (inner.clone(), inner, any::<bool>()).prop_map(|((ok_t, ok_v), (err_t, err_v), ok)| (
                SchemaType::result(ResultSpec {
                    ok: Some(Box::new(ok_t)),
                    err: Some(Box::new(err_t)),
                }),
                SchemaValue::Result(if ok {
                    ResultValuePayload::Ok {
                        value: Some(Box::new(ok_v)),
                    }
                } else {
                    ResultValuePayload::Err {
                        value: Some(Box::new(err_v)),
                    }
                }),
            )),
        ]
    })
}

/// A typed value whose root is a `Ref` to a record definition holding a matching pair.
fn matching_typed_value_strategy() -> impl Strategy<Value = TypedSchemaValue> {
    use golem_common::schema::*;
    matching_pair().prop_map(|(ty, value)| {
        let id = TypeId::new("root-def");
        let graph = SchemaGraph {
            defs: vec![golem_common::schema::SchemaTypeDef {
                id: id.clone(),
                name: Some("rootdef".into()),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "root".into(),
                    body: ty,
                    metadata: Default::default(),
                }]),
            }],
            root: SchemaType::ref_to(id),
        };
        let value = SchemaValue::Record {
            fields: vec![value],
        };
        assert_eq!(
            golem_common::schema::validation::validate_value(&graph, &graph.root, &value),
            Ok(()),
            "{value:?}"
        );
        TypedSchemaValue::new(graph, value)
    })
}

/// Every upper-case marker of a value, from its JSON text.
fn markers_of(value: &SchemaValue) -> Vec<String> {
    let re = regex::Regex::new(r"MRK[0-9]+Q").unwrap();
    let mut text = serde_json::to_string(value).unwrap();
    if let SchemaValue::Record { fields } = value {
        for field in fields {
            collect_bytes(field, &mut text);
        }
    }
    let mut out: Vec<String> = re
        .find_iter(&text)
        .map(|m| m.as_str().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Appends the text of byte payloads, which JSON holds as numbers.
fn collect_bytes(value: &SchemaValue, out: &mut String) {
    match value {
        SchemaValue::Binary(binary) => out.push_str(&String::from_utf8_lossy(&binary.bytes)),
        SchemaValue::List { elements }
            if !elements.is_empty() && elements.iter().all(|e| matches!(e, SchemaValue::U8(_))) =>
        {
            let bytes: Vec<u8> = elements
                .iter()
                .map(|e| match e {
                    SchemaValue::U8(b) => *b,
                    _ => 0,
                })
                .collect();
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
        SchemaValue::Record { fields: elements }
        | SchemaValue::Tuple { elements }
        | SchemaValue::List { elements }
        | SchemaValue::FixedList { elements } => {
            elements.iter().for_each(|e| collect_bytes(e, out))
        }
        SchemaValue::Map { entries } => entries.iter().for_each(|(k, v)| {
            collect_bytes(k, out);
            collect_bytes(v, out)
        }),
        SchemaValue::Variant(payload) => {
            if let Some(inner) = payload.payload.as_deref() {
                collect_bytes(inner, out)
            }
        }
        SchemaValue::Option { inner: Some(inner) } => collect_bytes(inner, out),
        SchemaValue::Result(
            golem_common::schema::ResultValuePayload::Ok { value: Some(inner) }
            | golem_common::schema::ResultValuePayload::Err { value: Some(inner) },
        ) => collect_bytes(inner, out),
        SchemaValue::Union(union) => collect_bytes(&union.body, out),
        _ => {}
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn no_marker_in_matching_typed_values(typed in matching_typed_value_strategy()) {
        let markers = markers_of(typed.value());
        for keep in all_keep() {
            let output = json(&render_typed(keep, &typed));
            prop_assert!(!output.contains("MRK"), "marker in {}", output);
            for marker in &markers {
                assert_no_marker(&output, marker);
            }
            prop_assert!(output.contains("\"name\":\"root\""), "{}", output);
        }
    }
}

/// The node fields that only a keep flag shows, by flag.
const SCALAR_FIELDS: [&str; 5] = ["value", "mantissa", "scale", "unit", "unitToken"];
const SHAPE_FIELDS: [&str; 7] = [
    "elements",
    "entries",
    "length",
    "case",
    "caseIndex",
    "payload",
    "tagToken",
];

/// The fields of customer value nodes (not kept metadata, records or tuples) in a rendered
/// value.
fn value_node_fields(node: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    match node {
        serde_json::Value::Object(map) => {
            if let Some(kind) = map.get("kind").and_then(|k| k.as_str())
                && !matches!(
                    kind,
                    "text" | "number" | "timestamp" | "flag" | "record" | "tuple"
                )
            {
                out.extend(map.keys().cloned());
            }
            map.values().for_each(|v| value_node_fields(v, out));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| value_node_fields(v, out)),
        _ => {}
    }
}

#[test]
fn value_fields_only_with_keep_flags() {
    let rich = rich_value();
    let mut scalars_graph = Vec::new();
    let mut scalars_value = Vec::new();
    for (i, (ty, value)) in [
        (SchemaType::u32(), SchemaValue::U32(7)),
        (SchemaType::f64(), SchemaValue::F64(1.5)),
        (SchemaType::bool(), SchemaValue::Bool(true)),
        (SchemaType::string(), SchemaValue::String("s".into())),
        (
            SchemaType::list(SchemaType::string()),
            SchemaValue::List {
                elements: vec![
                    SchemaValue::String("a".into()),
                    SchemaValue::String("b".into()),
                ],
            },
        ),
        (
            SchemaType::quantity(QuantitySpec {
                base_unit: "byte".into(),
                allowed_suffixes: Vec::new(),
                min: None,
                max: None,
            }),
            SchemaValue::Quantity(QuantityValue {
                mantissa: 5,
                scale: 0,
                unit: "byte".into(),
            }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        scalars_graph.push(named_field(i, ty));
        scalars_value.push(value);
    }
    let scalars = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(scalars_graph)),
        SchemaValue::Record {
            fields: scalars_value,
        },
    );
    let fields_with = |keep: KeepFlags| {
        let mut out = std::collections::BTreeSet::new();
        for typed in [&rich, &scalars] {
            for node in [render_typed(keep, typed), render(keep, typed.value())] {
                value_node_fields(&serde_json::to_value(&node).unwrap(), &mut out);
            }
        }
        out
    };
    let shown = |fields: &std::collections::BTreeSet<String>, names: &[&str]| {
        names
            .iter()
            .filter(|name| fields.contains(**name))
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    let none = fields_with(KeepFlags::default());
    assert!(shown(&none, &SCALAR_FIELDS).is_empty(), "{none:?}");
    assert!(shown(&none, &SHAPE_FIELDS).is_empty(), "{none:?}");
    let error_text = fields_with(KeepFlags {
        error_text: true,
        ..KeepFlags::default()
    });
    assert_eq!(error_text, none);
    let scalars_only = fields_with(KeepFlags {
        scalars: true,
        ..KeepFlags::default()
    });
    assert!(
        shown(&scalars_only, &SHAPE_FIELDS).is_empty(),
        "{scalars_only:?}"
    );
    assert_eq!(
        shown(&scalars_only, &SCALAR_FIELDS),
        ["value", "mantissa", "scale", "unit", "unitToken"]
    );
    let shape_only = fields_with(KeepFlags {
        shape: true,
        ..KeepFlags::default()
    });
    assert!(
        shown(&shape_only, &SCALAR_FIELDS).is_empty(),
        "{shape_only:?}"
    );
    assert_eq!(shown(&shape_only, &SHAPE_FIELDS), SHAPE_FIELDS);
}

#[test]
fn host_managed_values_redacted() {
    let secret = SchemaValue::Secret(golem_common::schema::SecretValuePayload {
        secret_id: Uuid::from_u128(99),
        config_key: Some(vec!["MARKERKEY".into()]),
        version: 1,
        resolved_at: chrono::Utc::now(),
        category: Some("MARKERCAT".into()),
    });
    let value = SchemaValue::Record {
        fields: vec![
            secret.clone(),
            SchemaValue::List {
                elements: vec![secret],
            },
        ],
    };
    for keep in all_keep() {
        let output = json(&render(keep, &value));
        assert!(output.contains("redacted"));
        assert_no_marker(&output, "MARKER");
        assert!(!output.contains(&Uuid::from_u128(99).to_string()));
    }
}

#[test]
fn agent_id_def_becomes_agent_node() {
    let id = agent_id("counter(\"MARKER\")");
    let typed = id.into_typed_schema_value().unwrap();
    let key = key();
    let ctx = ObfuscationContext::new(&key, KeepFlags::default());
    assert_eq!(
        render_typed(KeepFlags::default(), &typed),
        ReportNode::Agent(agent_reference(&ctx, &id))
    );
    let schemaless = render(KeepFlags::default(), &id.to_value());
    assert!(matches!(schemaless, ReportNode::Record(_)));
    assert_no_marker(&json(&schemaless), "MARKER");
}

#[test]
fn agent_reference_rules() {
    let key = key();
    let ctx = ObfuscationContext::new(&key, KeepFlags::default());
    let plain = agent_id("counter(\"MARKER\")");
    let phantom = agent_id(&format!("counter(\"MARKER\")[{}]", Uuid::from_u128(1)));
    let other_component = AgentId {
        component_id: ComponentId(Uuid::from_u128(2)),
        agent_id: plain.agent_id.clone(),
    };
    let a = agent_reference(&ctx, &plain);
    assert_ne!(a, agent_reference(&ctx, &phantom));
    assert_ne!(a, agent_reference(&ctx, &other_component));
    let output = json(&a);
    assert_no_marker(&output, "MARKER");
    assert!(!output.contains("counter"));
    assert!(!output.contains(&component_id().to_string()));
    assert_eq!(
        serde_json::to_value(&a).unwrap().as_object().unwrap().len(),
        1
    );
    let hex = a.token.hex().to_string();
    assert_eq!(a.pseudonym(), format!("agent:{}…", &hex[..12]));
    let legacy = agent_reference(&ctx, &agent_id("worker-1"));
    assert!(legacy.token.0.starts_with("agent:"));
}

#[test]
fn schema_types_in_customer_data_stay_data() {
    let graph = SchemaGraph::anonymous(SchemaType::record(vec![]));
    let mut typed = graph.into_typed_schema_value().unwrap();
    let mut value = typed.value().clone();
    let mut counter = 0;
    plant_markers(&mut value, &mut counter);
    typed = TypedSchemaValue::new(typed.graph().clone(), value);
    let output = json(&render_typed(KeepFlags::default(), &typed));
    assert!(!output.contains("typeDescription"));
    assert!(!output.contains("MRK"));
}

// -------------------------------------------------------------------------------------------
// Host paths
// -------------------------------------------------------------------------------------------

fn rpc_invoke(input: SchemaValue) -> HostRequest {
    HostRequest::GolemRpcInvoke(HostRequestGolemRpcInvoke {
        remote_agent_id: agent_id("callee(\"MARKERID\")"),
        idempotency_key: IdempotencyKey::new("MARKERKEY".into()),
        method_name: "do-it".into(),
        input,
        logical_streaming_origin: None,
        remote_agent_type: None,
        remote_agent_parameters: None,
        scope_card: None,
    })
}

#[test]
fn rpc_generic_values_reported() {
    let input = SchemaValue::String("MARKER".into());
    let invoke = start(
        5,
        fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>(),
        request(rpc_invoke(input.clone())),
    );
    let out = rendered(KeepFlags::default(), &invoke, None);
    let text = json(&out.entry);
    assert_no_marker(&text, "MARKER");
    assert!(text.contains("do-it"));
    assert!(matches!(
        record_field(field(&out.entry, "request"), "remoteAgentId"),
        ReportNode::Agent(_)
    ));
    assert_eq!(
        out.schema_less_rpc_values,
        vec![crate::oplog_export::model::SchemaLessRpcValueRef {
            index: 5,
            path: "request.input".into()
        }]
    );

    let result = end(
        6,
        response(HostResponse::GolemRpcInvokeAndAwait(
            HostResponseGolemRpcInvokeAndAwait { result: Ok(input) },
        )),
    );
    let out = rendered(
        KeepFlags::default(),
        &result,
        Some(fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>()),
    );
    assert_no_marker(&json(&out.entry), "MARKER");
    assert!(!out.provenance_unknown);
    assert_eq!(out.schema_less_rpc_values.len(), 1);
    assert_eq!(out.schema_less_rpc_values[0].path, "response.result.ok");
    let result_node = as_case(record_field(field(&out.entry, "response"), "result"));
    assert_eq!(result_node.case.as_deref(), Some("ok"));
}

#[test]
fn end_only_generic_rpc_with_and_without_start_function() {
    let input = SchemaValue::String("MARKER".into());
    let result = end(
        6,
        response(HostResponse::GolemRpcInvokeAndAwait(
            HostResponseGolemRpcInvokeAndAwait { result: Ok(input) },
        )),
    );
    let out = rendered(KeepFlags::default(), &result, None);
    assert_no_marker(&json(&out.entry), "MARKER");
    assert!(out.provenance_unknown);
    assert!(out.schema_less_rpc_values.is_empty());
    assert!(!json(&out.entry).contains("\"ok\""));
    let out = rendered(
        KeepFlags::default(),
        &result,
        Some(fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>()),
    );
    assert!(!out.provenance_unknown);
    assert_eq!(out.schema_less_rpc_values.len(), 1);
    assert_eq!(out.schema_less_rpc_values[0].path, "response.result.ok");

    let get = end(
        9,
        response(HostResponse::GolemRpcInvokeGet(
            HostResponseGolemRpcInvokeGet {
                result: SerializableInvokeResult::Completed(Ok(SchemaValue::String(
                    "MARKER".into(),
                ))),
            },
        )),
    );
    let out = rendered(KeepFlags::default(), &get, None);
    assert_no_marker(&json(&out.entry), "MARKER");
    assert!(out.provenance_unknown);
    assert!(out.schema_less_rpc_values.is_empty());
    let out = rendered(
        KeepFlags::default(),
        &get,
        Some(fqfn::<host_functions::GolemRpcFutureInvokeResultGet>()),
    );
    assert!(!out.provenance_unknown);
    assert_eq!(out.schema_less_rpc_values.len(), 1);
    assert_eq!(
        out.schema_less_rpc_values[0].path,
        "response.result.completed.ok"
    );
}

#[test]
fn rpc_generic_agent_id_stays_record() {
    let id = agent_id("counter(\"MARKER\")");
    let out = rendered(
        KeepFlags::default(),
        &start(
            5,
            fqfn::<host_functions::GolemRpcWasmRpcInvoke>(),
            request(rpc_invoke(id.to_value())),
        ),
        None,
    );
    let input = record_field(field(&out.entry, "request"), "input");
    assert!(matches!(input, ReportNode::Record(_)), "{input:?}");
    assert!(!json(input).contains("\"kind\":\"agent\""));
    assert_no_marker(&json(&out.entry), "MARKER");
    assert!(!json(&out.entry).contains(&component_id().to_string()));
}

#[test]
fn allowlist_stops_at_rpc_values() {
    let headers = SerializableResponseHeaders {
        status: 503,
        headers: HashMap::from([("x".into(), vec![b"MARKER".to_vec()])]),
    };
    let input = headers.to_value();
    let out = rendered(
        KeepFlags::default(),
        &start(
            5,
            fqfn::<host_functions::GolemRpcWasmRpcInvoke>(),
            request(rpc_invoke(input)),
        ),
        None,
    );
    let text = json(&out.entry);
    assert!(!text.contains("503"));
    assert_no_marker(&text, "MARKER");
}

#[test]
fn bare_entity_and_mcp_values_are_customer() {
    // A customer value whose graph is the graph of a trusted RPC response.
    let forged = response(HostResponse::GolemRpcInvokeAndAwait(
        HostResponseGolemRpcInvokeAndAwait {
            result: Err(SerializableRpcError::Denied {
                details: "MARKER".into(),
            }),
        },
    ));
    let entity_end = end(4, forged.clone());
    let out = rendered(
        KeepFlags::default(),
        &entity_end,
        Some(fqfn::<host_functions::GolemEntityInvoke>()),
    );
    let text = json(&out.entry);
    assert!(!text.contains("\"err\""));
    assert!(!text.contains("denied") && !text.contains("Denied"));
    assert_no_marker(&text, "MARKER");

    let entity_start = start(
        3,
        fqfn::<host_functions::GolemEntityInvoke>(),
        request(rpc_invoke(SchemaValue::Bool(true))),
    );
    let out = rendered(KeepFlags::default(), &entity_start, None);
    assert!(!json(&out.entry).contains("do-it"));
    assert!(out.schema_less_rpc_values.is_empty());

    // A real wrapper of the entity invocation response is unwrapped by the executor, so it is
    // never trusted either.
    let real = response(HostResponse::EntityInvocation(
        HostResponseEntityInvocation {
            result: Err("MARKER".into()),
        },
    ));
    let out = rendered(
        KeepFlags::default(),
        &end(4, real),
        Some(fqfn::<host_functions::GolemEntityInvoke>()),
    );
    assert_no_marker(&json(&out.entry), "MARKER");

    let mcp_start = start(
        3,
        fqfn::<host_functions::McpToolCall>(),
        request(rpc_invoke(SchemaValue::Bool(true))),
    );
    let out = rendered(KeepFlags::default(), &mcp_start, None);
    assert!(!json(&out.entry).contains("do-it"));
    assert!(out.schema_less_rpc_values.is_empty());

    let rejected_end = end(4, forged);
    let out = rendered(
        KeepFlags::default(),
        &rejected_end,
        Some(fqfn::<host_functions::GolemToolInvocationRejected>()),
    );
    let text = json(&out.entry);
    assert!(!text.contains("\"err\""));
    assert!(!text.to_lowercase().contains("denied"));
    assert_no_marker(&text, "MARKER");
}

#[test]
fn mcp_response_and_rejected_request_are_trusted() {
    use golem_common::base_model::tool::SerializableToolRpcError;
    let mcp = fqfn::<host_functions::McpToolCall>();
    let mcp_end = |result: Result<Vec<u8>, SerializableToolRpcError>, keep: KeepFlags| {
        let out = rendered(
            keep,
            &end(
                4,
                response(HostResponse::McpToolCall(
                    golem_common::model::oplog::HostResponseMcpToolCall { result },
                )),
            ),
            Some(mcp),
        );
        let result = as_case(record_field(field(&out.entry, "response"), "result")).clone();
        (json(&out.entry), result)
    };
    let denied = || Err(SerializableToolRpcError::Denied("MARKER".into()));
    let (text, result) = mcp_end(denied(), KeepFlags::default());
    assert_eq!(result.case.as_deref(), Some("err"));
    assert!(text.to_lowercase().contains("denied"), "{text}");
    assert_no_marker(&text, "MARKER");
    let (text, _) = mcp_end(
        denied(),
        KeepFlags {
            error_text: true,
            ..KeepFlags::default()
        },
    );
    assert!(text.contains("MARKER"));
    let (text, result) = mcp_end(Ok(b"MARKER".to_vec()), KeepFlags::default());
    assert_eq!(result.case.as_deref(), Some("ok"));
    assert_no_marker(&text, "MARKER");
    let Some(ReportNode::List(bytes)) = result.payload.as_deref() else {
        panic!("ok payload is not a list: {result:?}");
    };
    assert!(bytes.token.0.starts_with("list:"));

    let rejected = start(
        3,
        fqfn::<host_functions::GolemToolInvocationRejected>(),
        request(HostRequest::GolemToolInvocationRejected(
            golem_common::model::oplog::HostRequestGolemToolInvocationRejected {
                attempt_ordinal: 1,
                tool_name: "MRKTOOL".into(),
                command_path: vec!["MRKPATH".into()],
                input: Some(
                    SchemaValue::String("MRKINPUT".into())
                        .into_typed_schema_value()
                        .unwrap(),
                ),
                input_decode_failure: None,
                has_stdin: false,
                has_stdout: true,
                has_stderr: false,
                call_mode: golem_common::model::entity::EntityCallMode::Synchronous,
                error: SerializableToolRpcError::RemoteInternalError("MARKER".into()),
            },
        )),
    );
    for keep in all_keep() {
        let out = rendered(keep, &rejected, None);
        let text = json(&out.entry);
        let error = as_case(record_field(field(&out.entry, "request"), "error"));
        assert_eq!(
            super::value::normalized(error.case.as_deref().unwrap()),
            "remoteinternalerror"
        );
        for marker in ["MRKTOOL", "MRKPATH", "MRKINPUT"] {
            assert_no_marker(&text, marker);
        }
        assert_eq!(text.contains("MARKER"), keep.error_text, "{keep:?}: {text}");
    }
}

#[test]
fn end_only_http_with_and_without_start_function() {
    let value = response(HostResponse::P3HttpClientSendResult(
        HostResponseP3HttpClientSendResult {
            result: SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                headers: SerializableResponseHeaders {
                    status: 503,
                    headers: HashMap::from([("MARKERH".into(), vec![b"v".to_vec()])]),
                },
                recording_complete_at_end: true,
            },
        },
    ));
    let entry = end(8, value);
    let with = rendered(
        KeepFlags::default(),
        &entry,
        Some(fqfn::<host_functions::P3HttpClientSend>()),
    );
    let text = json(&with.entry);
    assert!(text.contains("503"));
    assert!(!with.provenance_unknown);
    assert_no_marker(&text, "MARKERH");
    let without = rendered(KeepFlags::default(), &entry, None);
    assert!(!json(&without.entry).contains("503"));
    assert!(without.provenance_unknown);
}

#[test]
fn provenance_unknown_only_for_end_and_cancelled_with_values() {
    let no_response = with_entry(
        2,
        PublicOplogEntry::End(EndParams {
            timestamp: Timestamp::from(0),
            start_index: OplogIndex::from_u64(1),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }),
    );
    assert!(!rendered(KeepFlags::default(), &no_response, None).provenance_unknown);
    let cancelled = |partial: Option<TypedSchemaValue>| {
        with_entry(
            2,
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp: Timestamp::from(0),
                start_index: OplogIndex::from_u64(1),
                partial,
                span_finished: None,
            }),
        )
    };
    assert!(!rendered(KeepFlags::default(), &cancelled(None), None).provenance_unknown);
    let partial = response(HostResponse::KVGet(HostResponseKVGet { result: Ok(None) }));
    assert!(rendered(KeepFlags::default(), &cancelled(Some(partial)), None).provenance_unknown);
    let start_entry = start(
        1,
        fqfn::<host_functions::RandomGetRandomBytes>(),
        request(HostRequest::RandomBytes(HostRequestRandomBytes {
            length: 1,
        })),
    );
    assert!(!rendered(KeepFlags::default(), &start_entry, None).provenance_unknown);
}

#[test]
fn forged_host_graph_is_customer() {
    // The graph of the real request type with one extra field: the type ID matches, the graph
    // does not.
    let mut graph =
        golem_common::schema::try_into_schema_graph::<HostRequestRandomBytes>().unwrap();
    let id = <HostRequestRandomBytes as IntoSchema>::type_id();
    let def = graph.defs.iter_mut().find(|def| def.id == id).unwrap();
    let SchemaType::Record { fields, .. } = &mut def.body else {
        panic!("not a record")
    };
    fields.push(NamedFieldType {
        name: "note".into(),
        body: SchemaType::string(),
        metadata: Default::default(),
    });
    let forged = TypedSchemaValue::new(
        graph,
        SchemaValue::Record {
            fields: vec![SchemaValue::U64(4711), SchemaValue::String("x".into())],
        },
    );
    let out = rendered(
        KeepFlags::default(),
        &start(1, fqfn::<host_functions::RandomGetRandomBytes>(), forged),
        None,
    );
    assert!(!json(&out.entry).contains("4711"));
}

#[test]
fn invoke_get_cases() {
    let get = fqfn::<host_functions::GolemRpcFutureInvokeResultGet>();
    let case_of = |result: SerializableInvokeResult, keep: KeepFlags| {
        let out = rendered(
            keep,
            &end(
                9,
                response(HostResponse::GolemRpcInvokeGet(
                    HostResponseGolemRpcInvokeGet { result },
                )),
            ),
            Some(get),
        );
        (
            json(&out.entry),
            as_case(record_field(field(&out.entry, "response"), "result")).clone(),
            out.schema_less_rpc_values,
        )
    };
    let keep = KeepFlags::default();
    let (text, case, rpc) = case_of(
        SerializableInvokeResult::Completed(Ok(SchemaValue::String("MARKER".into()))),
        keep,
    );
    assert_eq!(
        super::value::normalized(case.case.as_deref().unwrap()),
        "completed"
    );
    assert_no_marker(&text, "MARKER");
    assert_eq!(rpc[0].path, "response.result.completed.ok");
    let (text, _, _) = case_of(
        SerializableInvokeResult::Completed(Err(SerializableRpcError::Denied {
            details: "MARKER".into(),
        })),
        keep,
    );
    assert_no_marker(&text, "MARKER");
    assert!(text.to_lowercase().contains("denied"));
    for result in [
        SerializableInvokeResult::Failed("MARKER".into()),
        SerializableInvokeResult::FailedClassified {
            kind: SerializableHostFailureKind::Transient,
            message: "MARKER".into(),
        },
    ] {
        let (text, _, _) = case_of(result.clone(), keep);
        assert_no_marker(&text, "MARKER");
        let (text, _, _) = case_of(
            result,
            KeepFlags {
                error_text: true,
                ..keep
            },
        );
        assert!(text.contains("MARKER"));
    }
    let (text, _, _) = case_of(
        SerializableInvokeResult::Completed(Ok(SchemaValue::String("MARKER".into()))),
        KeepFlags {
            error_text: true,
            ..keep
        },
    );
    assert_no_marker(&text, "MARKER");
}

fn http_error(code: SerializableHttpErrorCode, keep: KeepFlags) -> String {
    json(
        &rendered(
            keep,
            &end(
                4,
                response(HostResponse::P3HttpClientSendResult(
                    HostResponseP3HttpClientSendResult {
                        result: SerializableP3HttpClientSendResult::HttpError(code),
                    },
                )),
            ),
            Some(fqfn::<host_functions::P3HttpClientSend>()),
        )
        .entry,
    )
}

#[test]
fn error_text_only_with_keep_error_text() {
    let kv = end(
        4,
        response(HostResponse::KVGet(HostResponseKVGet {
            result: Err("MARKER".into()),
        })),
    );
    let get = fqfn::<host_functions::KeyvalueEventualGet>();
    for keep in all_keep() {
        let text = json(&rendered(keep, &kv, Some(get)).entry);
        assert!(text.contains("\"err\""));
        assert_eq!(text.contains("MARKER"), keep.error_text, "{keep:?}: {text}");
    }
    let some = || SerializableHttpErrorCode::InternalError(Some("MARKER".into()));
    let none = || SerializableHttpErrorCode::InternalError(None);
    for keep in [
        KeepFlags::default(),
        KeepFlags {
            scalars: true,
            ..KeepFlags::default()
        },
        KeepFlags {
            error_text: true,
            ..KeepFlags::default()
        },
    ] {
        let a = http_error(some(), keep);
        assert!(a.to_lowercase().contains("internal"));
        assert_no_marker(&a, "MARKER");
        assert_eq!(
            without_tokens(&a),
            without_tokens(&http_error(none(), keep))
        );
    }
    let shape = KeepFlags {
        shape: true,
        ..KeepFlags::default()
    };
    assert_ne!(
        without_tokens(&http_error(some(), shape)),
        without_tokens(&http_error(none(), shape))
    );
    assert_no_marker(&http_error(some(), shape), "MARKER");
    assert!(
        http_error(
            some(),
            KeepFlags {
                shape: true,
                error_text: true,
                scalars: false
            }
        )
        .contains("MARKER")
    );
}

fn p2_fs_error(case: u32, keep: KeepFlags) -> String {
    let code = SerializableFsErrorCode::from_value(&SchemaValue::Enum { case }).unwrap();
    json(
        &rendered(
            keep,
            &end(
                4,
                response(HostResponse::FileSystemStat(HostResponseFileSystemStat {
                    result: Err(FileSystemError::ErrorCode(code)),
                })),
            ),
            Some(fqfn::<host_functions::FilesystemTypesDescriptorStat>()),
        )
        .entry,
    )
}

fn fs_error(error: SerializableP3FileSystemError, keep: KeepFlags) -> String {
    json(
        &rendered(
            keep,
            &end(
                4,
                response(HostResponse::P3FileSystemStat(
                    HostResponseP3FileSystemStat { result: Err(error) },
                )),
            ),
            Some(fqfn::<host_functions::P3FilesystemTypesDescriptorStat>()),
        )
        .entry,
    )
}

#[test]
fn nested_error_codes_kept() {
    let code = SerializableP3FileSystemError::ErrorCode;
    for keep in all_keep() {
        let access = fs_error(code(SerializableP3FsErrorCode::Access), keep);
        let busy = fs_error(code(SerializableP3FsErrorCode::Busy), keep);
        assert_ne!(without_tokens(&access), without_tokens(&busy), "{keep:?}");
        assert!(
            access.to_lowercase().contains("access"),
            "{keep:?}: {access}"
        );
        let generic = fs_error(
            SerializableP3FileSystemError::Generic("MARKER".into()),
            keep,
        );
        assert_eq!(
            generic.contains("MARKER"),
            keep.error_text,
            "{keep:?}: {generic}"
        );
        // The P2 code type is a schema enum, the P3 one a variant.
        let access = p2_fs_error(0, keep);
        let busy = p2_fs_error(4, keep);
        assert_ne!(without_tokens(&access), without_tokens(&busy), "{keep:?}");
        assert!(access.contains("access"), "{keep:?}: {access}");
        assert!(busy.contains("busy"), "{keep:?}: {busy}");
    }
}

#[test]
fn error_payload_shape_hidden_by_default() {
    use golem_common::model::oplog::types::SerializableFieldSizePayload;
    let pairs: Vec<(SerializableHttpErrorCode, SerializableHttpErrorCode)> = vec![
        (
            SerializableHttpErrorCode::InternalError(None),
            SerializableHttpErrorCode::InternalError(Some("text".into())),
        ),
        (
            SerializableHttpErrorCode::HttpRequestBodySize(None),
            SerializableHttpErrorCode::HttpRequestBodySize(Some(7)),
        ),
        (
            SerializableHttpErrorCode::HttpRequestHeaderSize(None),
            SerializableHttpErrorCode::HttpRequestHeaderSize(Some(SerializableFieldSizePayload {
                field_name: Some("name".into()),
                field_size: Some(3),
            })),
        ),
    ];
    for (none, some) in pairs {
        for keep in [
            KeepFlags::default(),
            KeepFlags {
                scalars: true,
                ..KeepFlags::default()
            },
            KeepFlags {
                error_text: true,
                ..KeepFlags::default()
            },
        ] {
            assert_eq!(
                without_tokens(&http_error(none.clone(), keep)),
                without_tokens(&http_error(some.clone(), keep)),
                "{keep:?}"
            );
        }
        let shape = KeepFlags {
            shape: true,
            ..KeepFlags::default()
        };
        assert_ne!(
            without_tokens(&http_error(none, shape)),
            without_tokens(&http_error(some, shape))
        );
    }
}

#[test]
fn error_subtree_customer_error_stays_customer() {
    let entry = end(
        4,
        response(HostResponse::GolemRpcInvokeAndAwait(
            HostResponseGolemRpcInvokeAndAwait {
                result: Err(SerializableRpcError::RemoteAgentError {
                    error: Box::new(golem_common::model::agent::AgentError::CustomError(
                        SchemaValue::String("MARKER".into())
                            .into_typed_schema_value()
                            .unwrap(),
                    )),
                }),
            },
        )),
    );
    let keep = KeepFlags {
        error_text: true,
        ..KeepFlags::default()
    };
    let text = json(
        &rendered(
            keep,
            &entry,
            Some(fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>()),
        )
        .entry,
    );
    assert!(text.to_lowercase().contains("customerror"), "{text}");
    assert_no_marker(&text, "MARKER");
}

#[test]
fn allowlist_random_and_fork() {
    let fork = start(
        2,
        fqfn::<host_functions::GolemApiForkWorker>(),
        request(HostRequest::GolemApiForkAgent(
            golem_common::model::oplog::HostRequestGolemApiForkAgent {
                source_agent_id: agent_id("a"),
                target_agent_id: agent_id("b"),
                oplog_index_cut_off: OplogIndex::from_u64(4711),
            },
        )),
    );
    let out = rendered(KeepFlags::default(), &fork, None);
    assert_eq!(
        record_field(field(&out.entry, "request"), "oplogIndexCutOff"),
        &super::value::number(4711)
    );
    let random = start(
        3,
        fqfn::<host_functions::RandomGetRandomBytes>(),
        request(HostRequest::RandomBytes(HostRequestRandomBytes {
            length: 6,
        })),
    );
    let out = rendered(KeepFlags::default(), &random, None);
    assert_eq!(
        record_field(field(&out.entry, "request"), "length"),
        &super::value::number(6)
    );
    let bytes = end(
        4,
        response(HostResponse::RandomBytes(
            golem_common::model::oplog::HostResponseRandomBytes {
                bytes: b"MARKER".to_vec(),
            },
        )),
    );
    for keep in all_keep() {
        let out = rendered(
            keep,
            &bytes,
            Some(fqfn::<host_functions::RandomGetRandomBytes>()),
        );
        let node = record_field(field(&out.entry, "response"), "bytes");
        let ReportNode::List(list) = node else {
            panic!("not a list: {node:?}");
        };
        assert!(list.token.0.starts_with("list:") && list.elements.is_none());
        assert_no_marker(&json(&out.entry), "MARKER");
    }
}

/// A record with an enum, flags, a union, an option, a byte list, a binary, a map and a
/// variant, each holding customer data.
fn rich_value() -> TypedSchemaValue {
    use golem_common::schema::*;
    let field = |name: &str, body: SchemaType| NamedFieldType {
        name: name.into(),
        body,
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(SchemaType::record(vec![
        field(
            "e",
            SchemaType::Enum {
                cases: vec!["a".into(), "b".into()],
                metadata: Default::default(),
            },
        ),
        field("f", SchemaType::flags(vec!["x".into(), "y".into()])),
        field(
            "u",
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "left".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix { prefix: "M".into() },
                    metadata: Default::default(),
                }],
            }),
        ),
        field("o", SchemaType::option(SchemaType::string())),
        field("l", SchemaType::list(SchemaType::u8())),
        field("bin", SchemaType::binary(BinaryRestrictions::default())),
        field(
            "m",
            SchemaType::map(SchemaType::string(), SchemaType::u32()),
        ),
        field(
            "v",
            SchemaType::variant(vec![
                VariantCaseType {
                    name: "none".into(),
                    payload: None,
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "some".into(),
                    payload: Some(SchemaType::string()),
                    metadata: Default::default(),
                },
            ]),
        ),
    ]));
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::Enum { case: 1 },
            SchemaValue::Flags {
                bits: vec![true, false],
            },
            SchemaValue::Union(UnionValuePayload {
                tag: "left".into(),
                body: Box::new(SchemaValue::String("MARKER1".into())),
            }),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("MARKER2".into()))),
            },
            SchemaValue::List {
                elements: b"MARKER3".iter().map(|b| SchemaValue::U8(*b)).collect(),
            },
            SchemaValue::Binary(BinaryValuePayload {
                bytes: b"MARKER4".to_vec(),
                mime_type: None,
            }),
            SchemaValue::Map {
                entries: vec![
                    (SchemaValue::String("MARKER5".into()), SchemaValue::U32(1)),
                    (SchemaValue::String("MARKER6".into()), SchemaValue::U32(2)),
                ],
            },
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::String("MARKER7".into()))),
            }),
        ],
    };
    TypedSchemaValue::new(graph, value)
}

/// The `tagToken` values of the union nodes in a rendered value.
fn tag_tokens(node: &ReportNode) -> Vec<String> {
    let re = regex::Regex::new(r#""tagToken":"([a-z:]+:[0-9a-f]{32})""#).unwrap();
    re.captures_iter(&json(node))
        .map(|c| c[1].to_string())
        .collect()
}

fn tokens(node: &ReportNode) -> Vec<String> {
    let re = regex::Regex::new(r"[a-z:]+:[0-9a-f]{32}").unwrap();
    re.find_iter(&json(node))
        .map(|m| m.as_str().to_string())
        .collect()
}

#[test]
fn rpc_typed_and_generic_give_same_tokens() {
    use golem_common::model::oplog::*;
    let typed = rich_value();
    let generic = typed.value().clone();
    let caller_start = start(
        5,
        fqfn::<host_functions::GolemRpcWasmRpcInvoke>(),
        request(rpc_invoke(generic.clone())),
    );
    let caller_end = end(
        6,
        response(HostResponse::GolemRpcInvokeAndAwait(
            HostResponseGolemRpcInvokeAndAwait {
                result: Ok(generic),
            },
        )),
    );
    let callee_start = with_entry(
        3,
        PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
            timestamp: Timestamp::from(0),
            invocation: PublicAgentInvocation::AgentMethodInvocation(
                AgentMethodInvocationParameters {
                    idempotency_key: IdempotencyKey::new("k".into()),
                    method_name: "do-it".into(),
                    function_input: typed.clone(),
                    trace_id: golem_common::model::invocation_context::TraceId::generate(),
                    trace_states: vec![],
                    invocation_context: vec![],
                },
            ),
            wallet_pin: golem_common::model::card::PublicInvocationWalletPin {
                wallet_token: golem_common::model::card::WalletVersionToken {
                    wallet_id_hash: [0; 32],
                    generation: 1,
                },
                scope_card_id: None,
            },
        }),
    );
    let callee_end = with_entry(
        4,
        PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
            timestamp: Timestamp::from(0),
            result: PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                output: typed,
            }),
            method_name: Some("do-it".into()),
            consumed_fuel: 0,
            component_revision: golem_common::model::component::ComponentRevision::new(1).unwrap(),
        }),
    );
    for keep in all_keep() {
        let caller_input = rendered(keep, &caller_start, None);
        let caller_input = record_field(field(&caller_input.entry, "request"), "input").clone();
        let caller_output = rendered(
            keep,
            &caller_end,
            Some(fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>()),
        );
        let ok = as_case(record_field(
            field(&caller_output.entry, "response"),
            "result",
        ))
        .payload
        .clone()
        .unwrap();
        let callee_input = rendered(keep, &callee_start, None);
        let invocation = as_case(field(&callee_input.entry, "invocation"))
            .payload
            .clone()
            .unwrap();
        let callee_input = record_field(&invocation, "functionInput").clone();
        let callee_output = rendered(keep, &callee_end, None);
        let result = as_case(field(&callee_output.entry, "result"))
            .payload
            .clone()
            .unwrap();
        let callee_output = record_field(&result, "output").clone();
        let expected = tokens(&callee_input);
        assert!(!expected.is_empty());
        // The generic value has no type to check its union tag against, so with
        // `--keep-shape` it carries a tag token where the typed value shows the tag.
        let caller_tags = tag_tokens(&caller_input);
        assert_eq!(caller_tags.len(), usize::from(keep.shape), "{keep:?}");
        assert_eq!(tag_tokens(&ok), caller_tags);
        assert!(tag_tokens(&callee_input).is_empty());
        let without_tags = |node: &ReportNode| {
            let tags = tag_tokens(node);
            tokens(node)
                .into_iter()
                .filter(|token| !tags.contains(token))
                .collect::<Vec<_>>()
        };
        assert_eq!(without_tags(&caller_input), expected, "{keep:?}");
        assert_eq!(without_tags(&ok), expected, "{keep:?}");
        assert_eq!(tokens(&callee_output), expected, "{keep:?}");
    }
}

#[test]
fn schema_as_data_drops_literals() {
    use golem_common::schema::*;
    let meta = || MetadataEnvelope {
        doc: Some("MARKER".into()),
        aliases: vec!["MARKER".into()],
        examples: vec!["MARKER".into()],
        deprecated: Some("MARKER".into()),
        role: Some(Role::Other("MARKER".into())),
    };
    let field = |name: &str, body: SchemaType| NamedFieldType {
        name: name.into(),
        body,
        metadata: meta(),
    };
    let expected_type = SchemaGraph::anonymous(SchemaType::record(vec![
        field(
            "title",
            SchemaType::text(TextRestrictions {
                languages: Some(vec!["MARKER".into()]),
                regex: Some("MARKER".into()),
                ..Default::default()
            })
            .with_metadata(meta()),
        ),
        field(
            "site",
            SchemaType::url(UrlRestrictions {
                allowed_schemes: Some(vec!["MARKER".into()]),
                allowed_hosts: Some(vec!["MARKER".into()]),
            }),
        ),
        field(
            "blob",
            SchemaType::binary(BinaryRestrictions {
                mime_types: Some(vec!["MARKER".into()]),
                ..Default::default()
            }),
        ),
        field(
            "either",
            SchemaType::union(UnionSpec {
                branches: vec![UnionBranch {
                    tag: "left".into(),
                    body: SchemaType::string(),
                    discriminator: DiscriminatorRule::Prefix {
                        prefix: "MARKER".into(),
                    },
                    metadata: meta(),
                }],
            }),
        ),
        field(
            "key",
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: Some("MARKER".into()),
            }),
        ),
        field(
            "quota",
            SchemaType::quota_token(QuotaTokenSpec {
                resource_name: Some("MARKER".into()),
            }),
        ),
        field(
            "parts",
            SchemaType::list(SchemaType::string()).with_metadata(MetadataEnvelope {
                role: Some(Role::Multimodal),
                ..Default::default()
            }),
        ),
        field(
            "size",
            SchemaType::quantity(QuantitySpec {
                base_unit: "byte".into(),
                allowed_suffixes: vec!["kib".into()],
                min: None,
                max: None,
            }),
        ),
    ]));
    let entry = start(
        6,
        fqfn::<host_functions::GolemAgentGetConfigValue>(),
        request(HostRequest::GolemAgentGetConfigValue(
            golem_common::model::oplog::HostRequestGolemAgentGetConfigValue {
                path: vec!["cfg".into()],
                expected_type,
            },
        )),
    );
    let text = json(&rendered(KeepFlags::default(), &entry, None).entry);
    assert_no_marker(&text, "MARKER");
    assert!(text.contains("typeDescription"));
    for name in [
        "\"title\"",
        "\"site\"",
        "\"blob\"",
        "\"either\"",
        "\"left\"",
        "\"key\"",
        "\"quota\"",
        "\"parts\"",
        "\"byte\"",
        "\"kib\"",
        "\"multimodal\"",
    ] {
        assert!(text.contains(name), "{name} missing in {text}");
    }
}

#[test]
fn unlisted_entry_fields_are_tokens() {
    use golem_common::model::oplog::*;
    let derived = |parents: u128, generation: u64| {
        with_entry(
            1,
            PublicOplogEntry::CardDerived(CardDerivedParams {
                timestamp: Timestamp::from(0),
                card_id: card(1),
                parent_ids: (0..parents).map(|n| card(10 + n)).collect(),
                wallet_generation: generation,
            }),
        )
    };
    let tool = |flags: bool| {
        let mut entry = derived(1, 7);
        entry.attribution = PublicOplogEntryAttribution::Entity(PublicEntityInvocationContext {
            invocation: PublicEntityInvocation {
                operation: Some(PublicEntityInvocationOperation::Tool(
                    PublicToolInvocationOperation {
                        command_path: vec!["c".into()],
                        has_stdin: flags,
                        has_stdout: flags,
                        declares_stdout: flags,
                        has_stderr: flags,
                        declares_stderr: flags,
                    },
                )),
                ..entity_invocation()
            },
            ancestors: vec![],
        });
        entry
    };
    let create = |plugins: usize| {
        let mut entry = every_entry_kind().remove(0);
        let PublicOplogEntry::Create(params) = &mut entry.entry else {
            panic!()
        };
        params.initial_active_plugins = (0..plugins)
            .map(|n| PluginInstallationDescription {
                plugin_priority: golem_common::model::component::PluginPriority(n as i32),
                ..marker_plugin()
            })
            .collect();
        entry
    };
    let shape = |a: &PublicOplogEntryWithIndex, b: &PublicOplogEntryWithIndex, keep| {
        (
            without_tokens(&json(&rendered(keep, a, None).entry)),
            without_tokens(&json(&rendered(keep, b, None).entry)),
        )
    };
    let pairs = [
        (derived(1, 7), derived(3, 7)),
        (derived(1, 7), derived(1, 8)),
        (tool(true), tool(false)),
        (create(1), create(3)),
    ];
    for (a, b) in &pairs {
        let (left, right) = shape(a, b, KeepFlags::default());
        assert_eq!(left, right);
    }
    // The keep flags show what the default hides.
    let scalars = KeepFlags {
        scalars: true,
        ..KeepFlags::default()
    };
    let (left, right) = shape(&pairs[1].0, &pairs[1].1, scalars);
    assert_ne!(left, right);
    let (left, right) = shape(&pairs[2].0, &pairs[2].1, scalars);
    assert_ne!(left, right);
    let with_shape = KeepFlags {
        shape: true,
        ..KeepFlags::default()
    };
    for (a, b) in [&pairs[0], &pairs[3]].map(|(a, b)| (a, b)) {
        let (left, right) = shape(a, b, with_shape);
        assert_ne!(left, right);
    }
}

fn key_ctx_token(domain: &str, bytes: &[u8]) -> String {
    let key = key();
    ObfuscationContext::new(&key, KeepFlags::default())
        .token(domain, bytes)
        .0
}

#[test]
fn schema_less_union_tag_with_shape() {
    let union = |tag: &str| {
        SchemaValue::Union(golem_common::schema::UnionValuePayload {
            tag: tag.into(),
            body: Box::new(SchemaValue::String("same".into())),
        })
    };
    let (left, right) = (union("left"), union("right"));
    let default = KeepFlags::default();
    assert_ne!(
        json(&render(default, &left)),
        json(&render(default, &right))
    );
    assert_eq!(
        without_tokens(&json(&render(default, &left))),
        without_tokens(&json(&render(default, &right)))
    );
    assert!(!json(&render(default, &left)).contains("left"));
    let shape = KeepFlags {
        shape: true,
        ..KeepFlags::default()
    };
    let ReportNode::Case(case) = render(shape, &left) else {
        panic!()
    };
    assert!(case.case.is_none() && case.payload.is_some());
    assert_eq!(
        case.tag_token.as_ref().map(|token| token.0.clone()),
        Some(key_ctx_token("str", &encode_str("left")))
    );
    assert!(!json(&render(shape, &left)).contains("left"));
    assert_ne!(json(&render(shape, &left)), json(&render(shape, &right)));
    // A typed union shows its declared tag.
    assert!(json(&render_typed(shape, &rich_value())).contains("\"case\":\"left\""));
}

fn registered_agent_type() -> golem_common::schema::RegisteredAgentTypeSchema {
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, RegisteredAgentTypeImplementer, Snapshotting,
    };
    use golem_common::schema::*;
    let parameter = |name: &str| NamedField {
        name: name.into(),
        source: Default::default(),
        schema: SchemaType::string(),
        metadata: MetadataEnvelope {
            doc: Some("MARKER".into()),
            ..Default::default()
        },
    };
    RegisteredAgentTypeSchema {
        agent_type: AgentTypeSchema {
            type_name: AgentTypeName("counter".into()),
            kind: AgentTypeKind::Regular,
            description: "MARKER".into(),
            source_language: "MARKER".into(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: Some("create".into()),
                description: "MARKER".into(),
                prompt_hint: Some("MARKER".into()),
                input_schema: InputSchema::parameters(vec![parameter("label")]),
            },
            methods: vec![AgentMethodSchema {
                name: "increment".into(),
                description: "MARKER".into(),
                prompt_hint: Some("MARKER".into()),
                input_schema: InputSchema::parameters(vec![parameter("step")]),
                output_schema: OutputSchema::Single(Box::new(SchemaType::u64())),
                http_endpoint: vec![],
                read_only: None,
            }],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        },
        implemented_by: RegisteredAgentTypeImplementer {
            component_id: component_id(),
            component_revision: golem_common::model::component::ComponentRevision::new(3).unwrap(),
            component_name: "MARKER".into(),
            account_id: golem_common::model::account::AccountId(secret_uuid(1)),
            account_email: golem_common::model::account::AccountEmail::new("MARKER@x"),
        },
    }
}

#[test]
fn registered_agent_type_is_described_on_host_paths() {
    let one = end(
        4,
        response(HostResponse::GolemAgentAgentType(
            golem_common::model::oplog::HostResponseGolemAgentAgentType {
                result: Ok(Some(registered_agent_type())),
            },
        )),
    );
    let all = end(
        4,
        response(HostResponse::GolemAgentAgentTypes(
            golem_common::model::oplog::HostResponseGolemAgentAgentTypes {
                result: Ok(vec![registered_agent_type(), registered_agent_type()]),
            },
        )),
    );
    for keep in all_keep() {
        for (entry, function) in [
            (&one, fqfn::<host_functions::GolemAgentGetAgentType>()),
            (&all, fqfn::<host_functions::GolemAgentGetAllAgentTypes>()),
        ] {
            let out = rendered(keep, entry, Some(function));
            let text = json(&out.entry);
            for name in [
                "\"counter\"",
                "\"create\"",
                "\"increment\"",
                "\"label\"",
                "\"step\"",
            ] {
                assert!(text.contains(name), "{keep:?}: {name} missing in {text}");
            }
            assert!(text.contains("typeDescription"));
            assert_no_marker(&text, "MARKER");
            assert!(!text.contains(&component_id().to_string()));
            assert!(!text.contains(&secret_uuid(1).to_string()));
        }
    }
    let none = end(
        4,
        response(HostResponse::GolemAgentAgentType(
            golem_common::model::oplog::HostResponseGolemAgentAgentType { result: Ok(None) },
        )),
    );
    let out = rendered(
        KeepFlags::default(),
        &none,
        Some(fqfn::<host_functions::GolemAgentGetAgentType>()),
    );
    let ok = as_case(record_field(field(&out.entry, "response"), "result"));
    let option = as_case(ok.payload.as_deref().unwrap());
    assert_eq!(option.case.as_deref(), Some("none"));

    // The same value in customer data is data.
    let typed = registered_agent_type().into_typed_schema_value().unwrap();
    let text = json(&render_typed(KeepFlags::default(), &typed));
    assert!(!text.contains("typeDescription"));
    assert!(!text.contains("counter") && !text.contains("increment"));
    assert_no_marker(&text, "MARKER");
}

#[test]
fn rpc_method_and_config_names_kept() {
    let out = rendered(
        KeepFlags::default(),
        &start(
            5,
            fqfn::<host_functions::GolemRpcWasmRpcScheduleInvocation>(),
            request(HostRequest::GolemRpcScheduledInvocation(
                golem_common::model::oplog::HostRequestGolemRpcScheduledInvocation {
                    remote_agent_id: agent_id("x"),
                    idempotency_key: IdempotencyKey::new("MARKERKEY".into()),
                    method_name: "scheduled-method".into(),
                    input: SchemaValue::Bool(true),
                    datetime: golem_common::model::oplog::types::SerializableDateTime {
                        seconds: 1,
                        nanoseconds: 0,
                    },
                    remote_agent_type: None,
                    remote_agent_parameters: None,
                },
            )),
        ),
        None,
    );
    let text = json(&out.entry);
    assert!(text.contains("scheduled-method"));
    assert_no_marker(&text, "MARKERKEY");
    let config = rendered(
        KeepFlags::default(),
        &start(
            6,
            fqfn::<host_functions::GolemAgentGetConfigValue>(),
            request(HostRequest::GolemAgentGetConfigValue(
                golem_common::model::oplog::HostRequestGolemAgentGetConfigValue {
                    path: vec!["db".into(), "url".into()],
                    expected_type: SchemaGraph::anonymous(SchemaType::string()),
                },
            )),
        ),
        None,
    );
    let text = json(&config.entry);
    assert!(text.contains("\"db\"") && text.contains("\"url\""));
    assert!(text.contains("typeDescription"));
}

#[test]
fn allowlist_http() {
    let method = |m: SerializableHttpMethod| {
        let key = key();
        let ctx = ObfuscationContext::new(&key, KeepFlags::default());
        let typed = m.into_typed_schema_value().unwrap();
        json(&Walker::new(&ctx).typed(
            typed.graph(),
            typed.root_type(),
            typed.value(),
            Region::Trusted,
        ))
    };
    assert!(
        method(SerializableHttpMethod::Get)
            .to_lowercase()
            .contains("get")
    );
    let other = method(SerializableHttpMethod::Other("MARKER".into()));
    assert!(other.to_lowercase().contains("other"));
    assert_no_marker(&other, "MARKER");
    let send = end(
        8,
        response(HostResponse::P3HttpClientSendResult(
            HostResponseP3HttpClientSendResult {
                result: SerializableP3HttpClientSendResult::SuccessWithRecordedRequestBody {
                    headers: SerializableResponseHeaders {
                        status: 503,
                        headers: HashMap::from([("MARKERH".into(), vec![b"MARKERV".to_vec()])]),
                    },
                    recording_complete_at_end: true,
                },
            },
        )),
    );
    let text = json(
        &rendered(
            KeepFlags::default(),
            &send,
            Some(fqfn::<host_functions::P3HttpClientSend>()),
        )
        .entry,
    );
    assert!(text.contains("\"value\":503"), "{text}");
    assert_no_marker(&text, "MARKERH");
    assert_no_marker(&text, "MARKERV");
}

#[test]
fn allowlist_is_keyed_by_path() {
    let graph = SchemaGraph::anonymous(SchemaType::record(vec![
        NamedFieldType {
            name: "status".into(),
            body: SchemaType::u16(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "length".into(),
            body: SchemaType::u64(),
            metadata: Default::default(),
        },
    ]));
    let typed = TypedSchemaValue::new(
        graph,
        SchemaValue::Record {
            fields: vec![SchemaValue::U16(503), SchemaValue::U64(4711)],
        },
    );
    let text = json(&render_typed(KeepFlags::default(), &typed));
    assert!(!text.contains("503") && !text.contains("4711"));
}

#[test]
fn component_ids_are_tokens() {
    let id = ComponentId(Uuid::from_u128(0xC0FFEE));
    let entry = start(
        2,
        fqfn::<host_functions::GolemApiGetAgents>(),
        request(HostRequest::GolemApiGetAgents(
            HostRequestGolemApiGetAgents { component_id: id },
        )),
    );
    for keep in [
        KeepFlags::default(),
        KeepFlags {
            scalars: true,
            shape: true,
            error_text: true,
        },
    ] {
        let text = json(&rendered(keep, &entry, None).entry);
        assert!(!text.contains(&id.0.hyphenated().to_string()));
        assert!(!text.contains(&id.0.simple().to_string()));
    }
}

#[test]
fn fingerprints_are_tokens() {
    let fingerprint = Uuid::from_u128(0xF1F1);
    let activate = end(
        4,
        response(HostResponse::GolemRpcActivate(
            HostResponseGolemRpcActivate {
                result: Ok(AgentFingerprint(fingerprint)),
            },
        )),
    );
    let out = rendered(
        KeepFlags::default(),
        &activate,
        Some(fqfn::<host_functions::GolemRpcWasmRpcActivate>()),
    );
    let text = json(&out.entry);
    assert!(text.contains("\"ok\""));
    assert!(!text.contains(&fingerprint.to_string()));
    let create = end(
        4,
        response(HostResponse::GolemRpcCreate(HostResponseGolemRpcCreate {
            target_fingerprint: AgentFingerprint(fingerprint),
            target_environment_id: EnvironmentId::new(),
        })),
    );
    let text = json(
        &rendered(
            KeepFlags::default(),
            &create,
            Some(fqfn::<host_functions::GolemRpcWasmRpcNew>()),
        )
        .entry,
    );
    assert!(!text.contains(&fingerprint.to_string()));
}

#[test]
fn schema_value_outside_rpc_not_reported() {
    let out = rendered(
        KeepFlags::default(),
        &end(
            4,
            response(HostResponse::GolemAgentGetConfigValue(
                HostResponseGolemAgentGetConfigValue {
                    result: Ok(SchemaValue::String("MARKER".into())),
                },
            )),
        ),
        Some(fqfn::<host_functions::GolemAgentGetConfigValue>()),
    );
    assert_no_marker(&json(&out.entry), "MARKER");
    assert!(out.schema_less_rpc_values.is_empty());
}

#[test]
fn rpc_other_cases_not_reported() {
    let out = rendered(
        KeepFlags::default(),
        &end(
            6,
            response(HostResponse::GolemRpcInvokeAndAwait(
                HostResponseGolemRpcInvokeAndAwait {
                    result: Err(SerializableRpcError::NotFound {
                        details: "x".into(),
                    }),
                },
            )),
        ),
        Some(fqfn::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>()),
    );
    assert!(out.schema_less_rpc_values.is_empty());
}

// -------------------------------------------------------------------------------------------
// Entries and error text
// -------------------------------------------------------------------------------------------

fn error_entry(error: &str) -> PublicOplogEntryWithIndex {
    with_entry(
        7,
        PublicOplogEntry::Error(ErrorParams {
            timestamp: Timestamp::from(0),
            kind: golem_common::model::oplog::OplogErrorKind::Invocation,
            error: error.to_string(),
            retry_from: OplogIndex::from_u64(3),
            retry_policy_state: None,
        }),
    )
}

#[test]
fn fixed_error_text_kept() {
    for keep in [
        KeepFlags::default(),
        KeepFlags {
            error_text: true,
            ..KeepFlags::default()
        },
    ] {
        let out = rendered(
            keep,
            &error_entry(&AgentError::StackOverflow.to_string("")),
            None,
        );
        assert_eq!(
            field(&out.entry, "error"),
            &super::value::text("Stack overflow")
        );
    }
}

#[test]
fn free_error_text_is_one_token() {
    let message = AgentError::TransientError("boom MARKER boom".into()).to_string("");
    let out = rendered(KeepFlags::default(), &error_entry(&message), None);
    let node = field(&out.entry, "error");
    assert!(matches!(node, ReportNode::Str(s) if s.token.is_some() && s.value.is_none()));
    assert_no_marker(&json(&out.entry), "MARKER");
    let kept = rendered(
        KeepFlags {
            error_text: true,
            ..KeepFlags::default()
        },
        &error_entry(&message),
        None,
    );
    assert!(json(field(&kept.entry, "error")).contains("MARKER"));
}

#[test]
fn fixed_text_exact_match_only() {
    for message in [
        "Stack overflow: MARKER",
        "Stack overflow\n\nMARKER",
        " Stack overflow",
        "stack overflow",
    ] {
        let out = rendered(KeepFlags::default(), &error_entry(message), None);
        assert!(matches!(field(&out.entry, "error"), ReportNode::Str(_)));
        assert_no_marker(&json(&out.entry), "MARKER");
        assert!(!json(&out.entry).contains("verflow"));
    }
}

#[test]
fn fixed_texts_cover_every_textless_variant() {
    let samples = agent_error_samples();
    let mut ordinals: Vec<usize> = samples.iter().map(agent_error_ordinal).collect();
    ordinals.sort();
    ordinals.dedup();
    assert_eq!(ordinals.len(), samples.len());
    // The variants of `AgentError`, read from its definition, so that a new variant without a
    // sample fails here.
    let source = include_str!("../../../../../golem-common/src/model/oplog/raw_types.rs");
    let body = source
        .split("pub enum AgentError {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .unwrap();
    let variant = regex::Regex::new(r"(?m)^    ([A-Z][A-Za-z0-9]*)\b").unwrap();
    let declared: std::collections::BTreeSet<String> = variant
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect();
    let sampled: std::collections::BTreeSet<String> = samples
        .iter()
        .map(|sample| {
            let debug = format!("{sample:?}");
            debug
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(sampled, declared);
    let expected: std::collections::BTreeSet<String> = [
        "Stack overflow",
        "Out of memory",
        "Exceeded plan memory limit",
        "Exceeded plan function table limit",
        "Exceeded per-invocation HTTP call limit",
        "Exceeded per-invocation RPC call limit",
        "Terminated by quota",
        "Ephemeral sleep too long",
        "Ephemeral fuel exhausted",
        "Ephemeral agent cannot suspend",
        "Read-only agent method attempted a side effect",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(*FIXED_ERROR_TEXTS, expected);
    for free in [
        AgentError::Unknown("MARKER".into()),
        AgentError::InvalidRequest("MARKER".into()),
        AgentError::InternalError("MARKER".into()),
        AgentError::DeterministicTrap("MARKER".into()),
        AgentError::TransientError("MARKER".into()),
        AgentError::PermanentError("MARKER".into()),
        AgentError::PermissionDenied("MARKER".into()),
    ] {
        assert_eq!(agent_error_text(&free), ErrorText::Free);
        assert!(!FIXED_ERROR_TEXTS.contains(free.message()));
    }
}

fn failed_update(details: &str, keep: KeepFlags) -> String {
    let entry = with_entry(
        4,
        PublicOplogEntry::FailedUpdate(FailedUpdateParams {
            timestamp: Timestamp::from(0),
            target_revision: golem_common::model::component::ComponentRevision::new(2).unwrap(),
            details: Some(details.to_string()),
            snapshot_assisted_details: None,
            update_attempt_index: None,
        }),
    );
    json(field(&rendered(keep, &entry, None).entry, "details"))
}

#[test]
fn failed_update_code_prefix() {
    let keep = KeepFlags::default();
    let prefixed = failed_update("UPDATE_SNAPSHOT_UNAVAILABLE: MARKER", keep);
    assert!(prefixed.contains("UPDATE_SNAPSHOT_UNAVAILABLE"));
    assert_no_marker(&prefixed, "MARKER");
    assert!(
        failed_update(
            "UPDATE_SNAPSHOT_UNAVAILABLE: MARKER",
            KeepFlags {
                error_text: true,
                ..keep
            }
        )
        .contains("MARKER")
    );
    assert_eq!(
        failed_update("UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS", keep),
        json(&super::value::text("UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS"))
    );
    for whole in ["UPDATE_SNAPSHOT_UNAVAILABLEMARKER", "SOMETHING: MARKER"] {
        let text = failed_update(whole, keep);
        assert!(!text.contains("UPDATE_") && !text.contains("SOMETHING"));
        assert_no_marker(&text, "MARKER");
    }
}

#[test]
fn failed_update_codes_match_executor() {
    let sources = [
        include_str!("../../../../../golem-worker-executor/src/worker/start_outcome/mod.rs"),
        include_str!("../../../../../golem-worker-executor/src/worker/filesystem_snapshots/mod.rs"),
    ];
    let re = regex::Regex::new(r#"const (UPDATE_[A-Z_]+): &str =\s*"([^"]*)""#).unwrap();
    let mut found = std::collections::BTreeSet::new();
    for source in sources {
        for capture in re.captures_iter(source) {
            if capture[1] == capture[2] {
                found.insert(capture[1].to_string());
            }
        }
    }
    let ours: std::collections::BTreeSet<String> = FAILED_UPDATE_DETAIL_CODES
        .iter()
        .map(|code| code.to_string())
        .collect();
    assert_eq!(found, ours);
}

#[test]
fn snapshot_data_removed() {
    let entry = with_entry(
        4,
        PublicOplogEntry::Snapshot(SnapshotParams {
            timestamp: Timestamp::from(0),
            data: golem_common::model::oplog::PublicSnapshotData::Raw(
                golem_common::model::oplog::RawSnapshotData {
                    data: b"MARKERDATA".to_vec(),
                    mime_type: "application/x-test".into(),
                },
            ),
            filesystem_snapshot: None,
        }),
    );
    let text = json(&rendered(KeepFlags::default(), &entry, None).entry);
    assert!(text.contains("application/x-test") && text.contains("10"));
    assert_no_marker(&text, "MARKERDATA");
    assert!(!text.contains("bytes:"));
}

#[test]
fn render_omitted_shape() {
    let key = key();
    let ctx = ObfuscationContext::new(&key, KeepFlags::default());
    let omitted = OmittedOplogEntry {
        oplog_index: OplogIndex::from_u64(42),
        attribution: PublicOplogEntryAttribution::agent(),
        timestamp: Timestamp::from(0),
        entry_type: "Snapshot".into(),
        size: 20_000_000,
        component_revision: golem_common::model::component::ComponentRevision::new(1).unwrap(),
    };
    let entry = render_omitted(&ctx, &omitted, OmittedReason::TooLarge);
    let value = serde_json::to_value(&entry).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "index": 42,
            "timestamp": serde_json::to_value(Timestamp::from(0)).unwrap(),
            "kind": "Snapshot",
            "omitted": { "reason": "tooLarge", "size": 20000000 }
        })
    );
    let failed = render_omitted(&ctx, &omitted, OmittedReason::HydrationFailed);
    assert_eq!(
        serde_json::to_value(&failed).unwrap()["omitted"]["reason"],
        "hydrationFailed"
    );
}

#[test]
fn report_entry_json_shape() {
    let entry = PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(3),
        attribution: PublicOplogEntryAttribution::entity(
            golem_common::model::oplog::PublicEntityInvocationContext {
                invocation: golem_common::model::oplog::PublicEntityInvocation {
                    entity: golem_common::model::oplog::PublicAgentEntity {
                        kind: golem_common::model::oplog::PublicAgentEntityKind::Tool,
                        name: "MARKERTOOL".into(),
                    },
                    start_index: OplogIndex::from_u64(2),
                    call_mode: golem_common::model::oplog::PublicEntityCallMode::Synchronous,
                    operation: None,
                },
                ancestors: vec![],
            },
        ),
        entry: start(
            3,
            fqfn::<host_functions::RandomGetRandomBytes>(),
            request(HostRequest::RandomBytes(HostRequestRandomBytes {
                length: 1,
            })),
        )
        .entry,
    };
    let out = rendered(KeepFlags::default(), &entry, None);
    let value = serde_json::to_value(&out.entry).unwrap();
    assert_eq!(value["kind"], "Start");
    assert!(value.get("omitted").is_none());
    assert_eq!(value["body"]["kind"], "record");
    let names: Vec<&str> = value["body"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["functionName", "request", "durableFunctionType"]);
    let durable = &value["body"]["fields"][2]["value"];
    assert_eq!(durable["kind"], "case");
    assert_eq!(durable["case"], "WriteRemote");
    assert!(durable.get("token").is_none());
    assert_eq!(value["attribution"]["kind"], "case");
    assert_eq!(value["attribution"]["case"], "Entity");
    assert_eq!(value["attribution"]["payload"]["kind"], "record");
    assert_no_marker(&value.to_string(), "MARKERTOOL");

    let invocation = with_entry(
        4,
        PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
            timestamp: Timestamp::from(0),
            invocation: golem_common::model::oplog::PublicAgentInvocation::AgentMethodInvocation(
                golem_common::model::oplog::AgentMethodInvocationParameters {
                    idempotency_key: IdempotencyKey::new("MARKERIK".into()),
                    method_name: "increment".into(),
                    function_input: SchemaValue::String("MARKERIN".into())
                        .into_typed_schema_value()
                        .unwrap(),
                    trace_id: golem_common::model::invocation_context::TraceId::generate(),
                    trace_states: vec!["MARKERTS".into()],
                    invocation_context: vec![],
                },
            ),
            wallet_pin: golem_common::model::card::PublicInvocationWalletPin {
                wallet_token: golem_common::model::card::WalletVersionToken {
                    wallet_id_hash: [7; 32],
                    generation: 1,
                },
                scope_card_id: None,
            },
        }),
    );
    let out = rendered(KeepFlags::default(), &invocation, None);
    let text = json(&out.entry);
    let case = as_case(field(&out.entry, "invocation"));
    assert_eq!(case.case.as_deref(), Some("AgentMethodInvocation"));
    assert!(text.contains("increment"));
    for marker in ["MARKERIK", "MARKERIN", "MARKERTS"] {
        assert_no_marker(&text, marker);
    }
}

/// One entry of the kinds that hold customer strings, each with a marker in every customer
/// field.
const TS: u64 = 1_700_000_000_123;
/// 2027-01-15T08:00:00Z, the time a scheduled invocation may start.
const NOT_BEFORE: u64 = 1_800_000_000_000;

#[test]
fn scheduled_start_is_a_datetime_token() {
    let entry = every_entry_kind()
        .into_iter()
        .find(|e| e.entry.entry_type() == "PendingAgentInvocation")
        .unwrap();
    let hidden = rendered(KeepFlags::default(), &entry, None);
    let ReportNode::Scalar(node) = field(&hidden.entry, "notBefore") else {
        panic!("notBefore is not a scalar");
    };
    assert!(node.value.is_none());
    assert!(node.token.0.starts_with("scalar:datetime:"));
    assert!(!json(&hidden.entry).contains("2027-01-15"));
    let shown = rendered(
        KeepFlags {
            scalars: true,
            ..KeepFlags::default()
        },
        &entry,
        None,
    );
    let ReportNode::Scalar(node) = field(&shown.entry, "notBefore") else {
        panic!("notBefore is not a scalar");
    };
    assert!(
        node.value
            .as_ref()
            .and_then(|v| v.as_str())
            .is_some_and(|v| v.starts_with("2027-01-15T08:00:00")),
        "{node:?}"
    );
}

/// A reader can predict numbers that Golem writes, such as consumed fuel or a resource ID, so
/// they must not share a token with an equal customer number.
#[test]
fn golem_numbers_do_not_name_customer_tokens() {
    let scalar_token = |node: &ReportNode| match node {
        ReportNode::Scalar(node) => node.token.0.clone(),
        other => panic!("not a scalar: {other:?}"),
    };
    let keep = KeepFlags::default();
    let finished = every_entry_kind()
        .into_iter()
        .find(|e| e.entry.entry_type() == "AgentInvocationFinished")
        .unwrap();
    let fuel = scalar_token(field(
        &rendered(keep, &finished, None).entry,
        "consumedFuel",
    ));
    let customer = scalar_token(&render(keep, &SchemaValue::U64(0x5EC2E7)));
    assert!(customer.starts_with("scalar:int:"), "{customer}");
    assert!(fuel.starts_with("golem:int:"), "{fuel}");
    assert_ne!(fuel[fuel.len() - 32..], customer[customer.len() - 32..]);

    let key = key();
    let ctx = ObfuscationContext::new(&key, keep);
    for value in [SchemaValue::U32(0), SchemaValue::Bool(false)] {
        let trusted = scalar_token(&Walker::new(&ctx).schemaless(&value, Region::Trusted));
        let error = scalar_token(&Walker::new(&ctx).schemaless(&value, Region::Error));
        let customer = scalar_token(&render(keep, &value));
        assert_eq!(trusted, error);
        assert_ne!(trusted, customer);
    }
}

/// IDs that must not appear in a report. Each ID field of `every_entry_kind` gets one.
fn secret_uuid(n: u128) -> Uuid {
    Uuid::from_u128(0x5ec2_e700_0000_0000_0000_0000_0000_0000 + n)
}

fn marker_value() -> TypedSchemaValue {
    SchemaValue::String("MARKER".into())
        .into_typed_schema_value()
        .unwrap()
}

fn marker_spans() -> Vec<Vec<golem_common::model::oplog::PublicSpanData>> {
    use golem_common::model::oplog::*;
    vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
        span_id: span(),
        start: Timestamp::from(TS),
        parent_id: None,
        linked_context: None,
        attributes: vec![marker_attribute()],
        inherited: false,
    })]]
}

fn marker_attribute() -> golem_common::model::oplog::PublicAttribute {
    use golem_common::model::oplog::*;
    PublicAttribute {
        key: "MARKER".into(),
        value: PublicAttributeValue::String(StringAttributeValue {
            value: "MARKER".into(),
        }),
    }
}

const SECRET_TRACE: &str = "5ec2e7005ec2e7005ec2e7005ec2e700";
const SECRET_SPAN: &str = "5ec2e7015ec2e701";

fn trace() -> golem_common::model::invocation_context::TraceId {
    golem_common::model::invocation_context::TraceId::from_string(SECRET_TRACE).unwrap()
}

fn span() -> golem_common::model::invocation_context::SpanId {
    golem_common::model::invocation_context::SpanId::from_string(SECRET_SPAN).unwrap()
}

fn marker_plugin() -> golem_common::model::oplog::PluginInstallationDescription {
    golem_common::model::oplog::PluginInstallationDescription {
        environment_plugin_grant_id:
            golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId(secret_uuid(1)),
        plugin_priority: golem_common::model::component::PluginPriority(0),
        plugin_name: "MARKER".into(),
        plugin_version: "MARKER".into(),
        parameters: std::collections::BTreeMap::from([(
            "MARKER".to_string(),
            "MARKER".to_string(),
        )]),
    }
}

fn marker_snapshot() -> golem_common::model::oplog::PublicSnapshotData {
    use golem_common::model::oplog::*;
    PublicSnapshotData::Multipart(MultipartSnapshotData {
        mime_type: "multipart/mixed".into(),
        parts: vec![
            MultipartSnapshotPart {
                name: "MARKER".into(),
                content_type: "application/json".into(),
                data: MultipartPartData::Json(JsonSnapshotData {
                    data: serde_json::json!({ "MARKER": "MARKER" }),
                }),
            },
            MultipartSnapshotPart {
                name: "MARKER".into(),
                content_type: "application/octet-stream".into(),
                data: MultipartPartData::Raw(RawSnapshotData {
                    data: b"MARKER".to_vec(),
                    mime_type: "application/octet-stream".into(),
                }),
            },
        ],
    })
}

fn marker_span_started() -> golem_common::model::oplog::PublicSpanStarted {
    use golem_common::model::oplog::*;
    PublicSpanStarted {
        span_id: span(),
        trace_id: trace(),
        trace_states: vec!["MARKER".into()],
        parent_span_id: Some(span()),
        links: vec![PublicSpanLink {
            trace_id: trace(),
            span_id: span(),
            trace_states: vec!["MARKER".into()],
        }],
        started_at: Timestamp::from(TS),
        attributes: vec![marker_attribute()],
        kind: PublicSpanKind::Client,
    }
}

fn marker_span_finished() -> golem_common::model::oplog::PublicSpanFinished {
    golem_common::model::oplog::PublicSpanFinished {
        span_id: span(),
        finished_at: Timestamp::from(TS),
        outcome: golem_common::model::oplog::PublicSpanOutcome::Failed,
    }
}

fn card(n: u128) -> golem_common::model::card::CardId {
    golem_common::model::card::CardId(secret_uuid(100 + n))
}

fn holders() -> Vec<golem_common::model::card::PublicCardHolder> {
    use golem_common::model::card::*;
    vec![
        CardHolder::Account(AccountCardHolder {
            account_id: secret_uuid(2),
        }),
        CardHolder::Application(ApplicationCardHolder {
            application_id: secret_uuid(3),
        }),
        CardHolder::Agent(AgentCardHolder {
            agent_id: agent_id("holder(\"MARKER\")"),
        }),
    ]
}

/// One entry of each public entry case (and of each invocation, result, update and card event
/// case), with `MARKER` in every customer string, a secret UUID in every ID and the component
/// ID of `component_id()` in every agent ID. Kept metadata uses plain lower-case names.
fn every_entry_kind() -> Vec<PublicOplogEntryWithIndex> {
    use golem_common::model::oplog::*;
    let ts = || Timestamp::from(TS);
    let marker_agent = agent_id("counter(\"MARKER\")");
    let key = || IdempotencyKey::new("MARKER".into());
    let revision = |n: u64| golem_common::model::component::ComponentRevision::new(n).unwrap();
    let invocations = vec![
        PublicAgentInvocation::AgentInitialization(AgentInitializationParameters {
            idempotency_key: key(),
            constructor_parameters: marker_value(),
            trace_id: trace(),
            trace_states: vec!["MARKER".into()],
            invocation_context: marker_spans(),
        }),
        PublicAgentInvocation::AgentMethodInvocation(AgentMethodInvocationParameters {
            idempotency_key: key(),
            method_name: "do-it".into(),
            function_input: marker_value(),
            trace_id: trace(),
            trace_states: vec!["MARKER".into()],
            invocation_context: marker_spans(),
        }),
        PublicAgentInvocation::ExternalTool(ExternalToolInvocationParameters {
            idempotency_key: key(),
            tool_name: "MARKER".into(),
            command_path: vec!["MARKER".into()],
            input: marker_value(),
            trace_id: trace(),
            trace_states: vec!["MARKER".into()],
            invocation_context: marker_spans(),
        }),
        PublicAgentInvocation::SaveSnapshot(Empty {}),
        PublicAgentInvocation::LoadSnapshot(LoadSnapshotParameters {
            snapshot: marker_snapshot(),
        }),
        PublicAgentInvocation::ProcessOplogEntries(ProcessOplogEntriesParameters {
            idempotency_key: key(),
        }),
        PublicAgentInvocation::ManualUpdate(ManualUpdateParameters {
            target_revision: revision(5),
        }),
    ];
    let results = vec![
        PublicAgentInvocationResult::AgentInitialization(AgentInvocationOutputParameters {
            output: marker_value(),
        }),
        PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
            output: marker_value(),
        }),
        PublicAgentInvocationResult::ExternalTool(ExternalToolResultParameters {
            result: PublicExternalToolResult::Success(
                golem_common::base_model::tool::SerializableToolInvocationResult {
                    result: Some(Box::new(marker_value())),
                },
            ),
        }),
        PublicAgentInvocationResult::ManualUpdate(Empty {}),
        PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters {
            error: Some("MARKER".into()),
        }),
        PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
            snapshot: marker_snapshot(),
        }),
        PublicAgentInvocationResult::ProcessOplogEntries(ProcessOplogEntriesResultParameters {
            error: Some("MARKER".into()),
        }),
    ];
    let updates = vec![
        PublicUpdateDescription::Automatic(Empty {}),
        PublicUpdateDescription::SnapshotAssistedAutomatic(
            SnapshotAssistedAutomaticUpdateParameters {},
        ),
        PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
            payload: b"MARKER".to_vec(),
            mime_type: "application/octet-stream".into(),
            filesystem_snapshot: Some("MARKER".into()),
        }),
    ];
    let mut card_events = vec![
        PublicQueuedCardEvent::Install(PublicQueuedCardEventCard { card_id: card(1) }),
        PublicQueuedCardEvent::Revoke(PublicQueuedCardEventCard { card_id: card(2) }),
        PublicQueuedCardEvent::TransferReceived(PublicQueuedCardEventTransferReceived {
            transfer_id: secret_uuid(4),
            card_id: card(3),
        }),
    ];
    for holder in holders() {
        card_events.push(PublicQueuedCardEvent::TransferStarted(
            PublicQueuedCardEventTransfer {
                transfer_id: secret_uuid(5),
                card_id: card(4),
                target_holder: holder,
            },
        ));
    }

    let mut entries = vec![
        PublicOplogEntry::Create(CreateParams {
            timestamp: ts(),
            agent_id: marker_agent.clone(),
            owner_kind: Default::default(),
            agent_mode: golem_common::model::agent::AgentMode::Durable,
            component_revision: revision(1),
            env: [("MY_ENV".to_string(), "MARKER".to_string())].into(),
            created_by: golem_common::model::account::AccountId(secret_uuid(6)),
            environment_id: EnvironmentId(secret_uuid(7)),
            parent: Some(marker_agent.clone()),
            component_size: 10,
            initial_total_linear_memory_size: 20,
            initial_active_plugins: std::collections::BTreeSet::from([marker_plugin()]),
            local_agent_config: vec![PublicTypedAgentConfigEntry {
                path: vec!["cfg".into()],
                value: marker_value(),
            }],
            original_phantom_id: Some(secret_uuid(8)),
            instance_id: secret_uuid(9),
        }),
        PublicOplogEntry::Start(StartParams {
            timestamp: ts(),
            parent_start_index: Some(OplogIndex::from_u64(1)),
            function_name: fqfn::<host_functions::GolemRpcWasmRpcInvoke>().to_string(),
            invocation_id: Some(DurableInvocationId::new(OplogIndex::from_u64(1), 3)),
            observational_owner: Some(OplogIndex::from_u64(1)),
            request: Some(request(rpc_invoke(SchemaValue::String("MARKER".into())))),
            durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
            span_started: Some(marker_span_started()),
            denial: Some("MARKER".into()),
        }),
        PublicOplogEntry::End(EndParams {
            timestamp: ts(),
            start_index: OplogIndex::from_u64(2),
            response: Some(marker_value()),
            forced_commit: true,
            span_finished: Some(marker_span_finished()),
            span_attributes: Some(PublicSpanAttributes {
                span_id: span(),
                attributes: vec![marker_attribute()],
            }),
        }),
        PublicOplogEntry::Cancelled(CancelledParams {
            timestamp: ts(),
            start_index: OplogIndex::from_u64(2),
            partial: Some(marker_value()),
            span_finished: Some(marker_span_finished()),
        }),
        PublicOplogEntry::Suspend(SuspendParams { timestamp: ts() }),
        PublicOplogEntry::Error(ErrorParams {
            timestamp: ts(),
            kind: OplogErrorKind::Invocation,
            error: "MARKER".into(),
            retry_from: OplogIndex::from_u64(3),
            retry_policy_state: Some(PublicRetryPolicyState::Counter(
                PublicRetryPolicyStateCounter { count: 2 },
            )),
        }),
        PublicOplogEntry::RecoverySucceeded(RecoverySucceededParams { timestamp: ts() }),
        PublicOplogEntry::Jump(JumpParams {
            timestamp: ts(),
            jump: golem_common::model::regions::OplogRegion {
                start: OplogIndex::from_u64(1),
                end: OplogIndex::from_u64(2),
            },
        }),
        PublicOplogEntry::Interrupted(InterruptedParams { timestamp: ts() }),
        PublicOplogEntry::Exited(ExitedParams { timestamp: ts() }),
        PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
            timestamp: ts(),
            target_revision: revision(2),
            new_component_size: 30,
            new_active_plugins: std::collections::BTreeSet::from([marker_plugin()]),
            snapshot_assisted_details: Some(PublicSnapshotAssistedUpdateDetails {
                pending_update_index: OplogIndex::from_u64(3),
                source_component_revision: revision(1),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: OplogIndex::from_u64(2),
            }),
        }),
        PublicOplogEntry::FailedUpdate(FailedUpdateParams {
            timestamp: ts(),
            target_revision: revision(3),
            details: Some("MARKER".into()),
            update_attempt_index: Some(OplogIndex::from_u64(20)),
            snapshot_assisted_details: Some(PublicFailedSnapshotAssistedUpdateDetails {
                pending_update_index: OplogIndex::from_u64(21),
                source_component_revision: revision(2),
                source_revision_start_index: OplogIndex::INITIAL,
                snapshot_index: OplogIndex::from_u64(19),
            }),
        }),
        PublicOplogEntry::GrowMemory(GrowMemoryParams {
            timestamp: ts(),
            delta: 64,
        }),
        PublicOplogEntry::CreateResource(CreateResourceParams {
            timestamp: ts(),
            id: AgentResourceId(0x5EC2E7),
            name: "resource".into(),
            owner: "owner".into(),
        }),
        PublicOplogEntry::DropResource(DropResourceParams {
            timestamp: ts(),
            id: AgentResourceId(0x5EC2E7),
            name: "resource".into(),
            owner: "owner".into(),
        }),
        PublicOplogEntry::Log(LogParams {
            timestamp: ts(),
            level: LogLevel::Warn,
            context: "MARKER".into(),
            message: "MARKER".into(),
            trace_context: Some(LogTraceContext {
                trace_id: trace(),
                span_id: span(),
            }),
        }),
        PublicOplogEntry::Restart(RestartParams { timestamp: ts() }),
        PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
            timestamp: ts(),
            plugin: marker_plugin(),
        }),
        PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
            timestamp: ts(),
            plugin: marker_plugin(),
        }),
        PublicOplogEntry::Revert(RevertParams {
            timestamp: ts(),
            dropped_region: golem_common::model::regions::OplogRegion {
                start: OplogIndex::from_u64(5),
                end: OplogIndex::from_u64(10),
            },
        }),
        PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
            timestamp: ts(),
            idempotency_key: key(),
        }),
        PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
            timestamp: ts(),
            transaction_id: golem_common::model::TransactionId::new("MARKER"),
        }),
        PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
            timestamp: ts(),
            begin_index: OplogIndex::from_u64(11),
        }),
        PublicOplogEntry::PreRollbackRemoteTransaction(PreRollbackRemoteTransactionParams {
            timestamp: ts(),
            begin_index: OplogIndex::from_u64(11),
        }),
        PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
            timestamp: ts(),
            begin_index: OplogIndex::from_u64(11),
        }),
        PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
            timestamp: ts(),
            begin_index: OplogIndex::from_u64(11),
        }),
        PublicOplogEntry::Snapshot(SnapshotParams {
            timestamp: ts(),
            data: marker_snapshot(),
            filesystem_snapshot: Some("MARKER".into()),
        }),
        PublicOplogEntry::OplogProcessorCheckpoint(OplogProcessorCheckpointParams {
            timestamp: ts(),
            plugin: marker_plugin(),
            target_agent_id: marker_agent.clone(),
            confirmed_up_to: OplogIndex::from_u64(20),
            sending_up_to: OplogIndex::from_u64(21),
            last_batch_start: OplogIndex::from_u64(19),
        }),
        PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
            timestamp: ts(),
            policy: PublicNamedRetryPolicy {
                name: "MARKER".into(),
                priority: 0x5EC2E7,
                predicate: golem_common::base_model::retry_policy::ApiPredicate::True(
                    golem_common::base_model::retry_policy::ApiPredicateTrue {},
                ),
                policy: golem_common::base_model::retry_policy::ApiRetryPolicy::Immediate(
                    golem_common::base_model::retry_policy::ApiImmediatePolicy {},
                ),
            },
        }),
        PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams {
            timestamp: ts(),
            name: "MARKER".into(),
        }),
        PublicOplogEntry::CardInstalled(CardInstalledParams {
            timestamp: ts(),
            queued_event_index: Some(OplogIndex::from_u64(4)),
            card_id: card(5),
            wallet_generation: 7,
        }),
        PublicOplogEntry::CardInstallFailed(CardInstallFailedParams {
            timestamp: ts(),
            queued_event_index: OplogIndex::from_u64(4),
            card_id: card(6),
            reason: CardInstallFailure::NotPermitted,
        }),
        PublicOplogEntry::CardRevoked(CardRevokedParams {
            timestamp: ts(),
            queued_event_index: OplogIndex::from_u64(4),
            card_id: card(7),
            wallet_generation: 8,
        }),
        PublicOplogEntry::CardExpired(CardExpiredParams {
            timestamp: ts(),
            card_id: card(8),
            wallet_generation: 9,
        }),
        PublicOplogEntry::CardDerived(CardDerivedParams {
            timestamp: ts(),
            card_id: card(9),
            parent_ids: vec![card(10)],
            wallet_generation: 10,
        }),
        PublicOplogEntry::CardRevokedCascade(CardRevokedCascadeParams {
            timestamp: ts(),
            revoked_card_ids: vec![card(11)],
            local_wallet_generation: 11,
        }),
        PublicOplogEntry::HostStreamFrame(HostStreamFrameParams {
            timestamp: ts(),
            parent_start_index: OplogIndex::from_u64(2),
            kind: HostStreamKind::P3HttpRequestBody,
            payload: marker_value(),
        }),
        PublicOplogEntry::ToolAttachmentRead(ToolAttachmentReadParams {
            timestamp: ts(),
            observation: marker_value(),
            payload: Some(b"MARKER".to_vec()),
        }),
        PublicOplogEntry::ToolAttachmentControl(ToolAttachmentControlParams {
            timestamp: ts(),
            attachment: marker_value(),
            transition: marker_value(),
        }),
        PublicOplogEntry::StreamRegistered(StreamRegisteredParams {
            timestamp: ts(),
            record: marker_value(),
        }),
        PublicOplogEntry::StreamItems(StreamItemsParams {
            timestamp: ts(),
            record: marker_value(),
        }),
        PublicOplogEntry::StreamEnd(StreamEndParams {
            timestamp: ts(),
            record: marker_value(),
        }),
        PublicOplogEntry::StreamCancel(StreamCancelParams {
            timestamp: ts(),
            record: marker_value(),
        }),
        PublicOplogEntry::StreamSession(StreamSessionParams {
            timestamp: ts(),
            record: marker_value(),
        }),
        PublicOplogEntry::CompletionDiscarded(CompletionDiscardedParams {
            timestamp: ts(),
            start_index: OplogIndex::from_u64(2),
        }),
        PublicOplogEntry::CompletionDelivered(CompletionDeliveredParams {
            timestamp: ts(),
            start_index: OplogIndex::from_u64(2),
        }),
        PublicOplogEntry::SnapshotConfirmed(SnapshotConfirmedParams {
            timestamp: ts(),
            filesystem_snapshot: "MARKER".into(),
        }),
        PublicOplogEntry::SpanClosed(SpanClosedParams {
            timestamp: ts(),
            resource_start_index: Some(OplogIndex::from_u64(12)),
            span_finished: marker_span_finished(),
        }),
        PublicOplogEntry::Resumed(ResumedParams { timestamp: ts() }),
    ];
    for invocation in invocations {
        entries.push(PublicOplogEntry::AgentInvocationStarted(
            AgentInvocationStartedParams {
                timestamp: ts(),
                invocation: invocation.clone(),
                wallet_pin: golem_common::model::card::PublicInvocationWalletPin {
                    wallet_token: golem_common::model::card::WalletVersionToken {
                        wallet_id_hash: *b"MARKERMARKERMARKERMARKERMARKER__",
                        generation: 1,
                    },
                    scope_card_id: Some(card(12)),
                },
            },
        ));
        entries.push(PublicOplogEntry::PendingAgentInvocation(
            PendingAgentInvocationParams {
                timestamp: ts(),
                invocation,
                not_before: Some(Timestamp::from(NOT_BEFORE)),
            },
        ));
    }
    for result in results {
        entries.push(PublicOplogEntry::AgentInvocationFinished(
            AgentInvocationFinishedParams {
                timestamp: ts(),
                result,
                method_name: Some("do-it".into()),
                consumed_fuel: 0x5EC2E7,
                component_revision: revision(4),
            },
        ));
    }
    for (n, description) in updates.into_iter().enumerate() {
        entries.push(PublicOplogEntry::PendingUpdate(PendingUpdateParams {
            timestamp: ts(),
            target_revision: revision(2),
            update_attempt_index: OplogIndex::from_u64(17 + n as u64),
            description,
        }));
    }
    for event in card_events {
        entries.push(PublicOplogEntry::CardEventQueued(CardEventQueuedParams {
            timestamp: ts(),
            event,
        }));
    }
    for holder in holders() {
        entries.push(PublicOplogEntry::CardTransferStarted(
            CardTransferStartedParams {
                timestamp: ts(),
                transfer_id: secret_uuid(10),
                card_id: card(13),
                target_holder: holder.clone(),
                source_wallet_generation: 12,
            },
        ));
        entries.push(PublicOplogEntry::CardTransferred(CardTransferredParams {
            timestamp: ts(),
            transfer_id: secret_uuid(11),
            source_card_id: card(14),
            installed_card_id: card(15),
            target_holder: holder.clone(),
            target_wallet_generation: 13,
        }));
        entries.push(PublicOplogEntry::CardTransferConfirmed(
            CardTransferConfirmedParams {
                timestamp: ts(),
                transfer_id: secret_uuid(12),
                source_card_id: card(16),
                installed_card_id: card(17),
                target_holder: holder,
            },
        ));
    }
    let mut out: Vec<_> = entries
        .into_iter()
        .enumerate()
        .map(|(n, entry)| with_entry(n as u64 + 1, entry))
        .collect();
    // The JSON form of a tool failure repeats the `type` tag, so this entry skips the JSON
    // round trip of `with_entry`.
    out.push(PublicOplogEntryWithIndex {
        oplog_index: OplogIndex::from_u64(out.len() as u64 + 1),
        attribution: PublicOplogEntryAttribution::agent(),
        entry: PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
            timestamp: ts(),
            result: PublicAgentInvocationResult::ExternalTool(ExternalToolResultParameters {
                result: PublicExternalToolResult::Failure(
                    golem_common::base_model::tool::SerializableToolRpcError::Denied(
                        "MARKER".into(),
                    ),
                ),
            }),
            method_name: Some("do-it".into()),
            consumed_fuel: 0x5EC2E7,
            component_revision: revision(4),
        }),
    });
    // One entry attributed to an entity.

    let mut attributed = out[1].clone();
    attributed.attribution = PublicOplogEntryAttribution::Entity(PublicEntityInvocationContext {
        invocation: entity_invocation(),
        ancestors: vec![entity_invocation()],
    });
    out.push(serde_json::from_str(&serde_json::to_string(&attributed).unwrap()).unwrap());
    out
}

fn entity_invocation() -> golem_common::model::oplog::PublicEntityInvocation {
    use golem_common::model::oplog::*;
    PublicEntityInvocation {
        entity: PublicAgentEntity {
            kind: PublicAgentEntityKind::Tool,
            name: "MARKER".into(),
        },
        start_index: OplogIndex::from_u64(2),
        call_mode: PublicEntityCallMode::Synchronous,
        operation: Some(PublicEntityInvocationOperation::Tool(
            PublicToolInvocationOperation {
                command_path: vec!["MARKER".into()],
                has_stdin: true,
                has_stdout: false,
                declares_stdout: true,
                has_stderr: false,
                declares_stderr: false,
            },
        )),
    }
}

/// Texts that every rendered entry of `every_entry_kind` must not hold.
fn every_entry_kind_secrets() -> Vec<String> {
    let mut secrets = vec![
        SECRET_TRACE.to_string(),
        SECRET_SPAN.to_string(),
        component_id().to_string(),
        component_id().0.simple().to_string(),
        // `AgentResourceId`, retry priority and consumed fuel.
        0x5EC2E7u64.to_string(),
        // The scheduled start of a pending invocation.
        "2027-01-15".to_string(),
    ];
    for n in 0..120 {
        secrets.push(secret_uuid(n).hyphenated().to_string());
        secrets.push(secret_uuid(n).simple().to_string());
    }
    secrets
}

#[test]
fn every_entry_kind_has_no_marker() {
    let entries = every_entry_kind();
    let kinds: std::collections::BTreeSet<_> = entries
        .iter()
        .map(|e| e.entry.entry_type().to_string())
        .collect();
    assert_eq!(kinds.len(), 57, "{kinds:?}");
    let secrets = every_entry_kind_secrets();
    for entry in &entries {
        let out = rendered(KeepFlags::default(), entry, None);
        let text = json(&out.entry);
        assert_no_marker(&text, "MARKER");
        for secret in &secrets {
            assert!(!text.contains(secret.as_str()), "{secret} in {text}");
        }
        assert!(!text.contains("counter("), "{text}");
    }
    let create = rendered(KeepFlags::default(), &entries[0], None);
    assert!(matches!(
        field(&create.entry, "instanceId"),
        ReportNode::Str(_)
    ));
}

#[test]
fn every_entry_kind_keeps_metadata() {
    let entries = every_entry_kind();
    let render = |kind: &str| {
        let entry = entries
            .iter()
            .find(|e| e.entry.entry_type() == kind)
            .unwrap_or_else(|| panic!("no {kind}"));
        rendered(KeepFlags::default(), entry, None)
    };
    for (n, entry) in entries.iter().enumerate() {
        let out = rendered(KeepFlags::default(), entry, None);
        assert_eq!(out.entry.index, entry.oplog_index.as_u64());
        assert_eq!(out.entry.timestamp, Timestamp::from(TS));
        assert_eq!(out.entry.kind, entry.entry.entry_type().to_string(), "{n}");
    }
    let create = render("Create");
    let text = json(&create.entry);
    assert!(text.contains("\"MY_ENV\"") && text.contains("\"cfg\""));
    assert!(matches!(
        field(&create.entry, "agentId"),
        ReportNode::Agent(_)
    ));
    assert_eq!(
        field(&create.entry, "componentRevision"),
        &super::value::number(1)
    );
    assert_eq!(
        field(&create.entry, "componentSize"),
        &super::value::number(10)
    );
    for (name, value) in [
        (
            "ownerKind",
            serde_json::to_value(golem_common::model::agent::OwnerKind::default()),
        ),
        (
            "agentMode",
            serde_json::to_value(golem_common::model::agent::AgentMode::Durable),
        ),
    ] {
        assert_eq!(
            field(&create.entry, name),
            &super::value::text(value.unwrap().as_str().unwrap())
        );
    }
    let start = render("Start");
    let span_kind =
        serde_json::to_value(golem_common::model::oplog::PublicSpanKind::Client).unwrap();
    assert_eq!(
        record_field(field(&start.entry, "spanStarted"), "kind"),
        &super::value::text(span_kind.as_str().unwrap())
    );
    assert_eq!(
        field(&start.entry, "functionName"),
        &super::value::text(fqfn::<host_functions::GolemRpcWasmRpcInvoke>())
    );
    assert!(json(&start.entry).contains("\"do-it\""));
    assert_eq!(
        field(&start.entry, "parentStartIndex"),
        &super::value::number(1)
    );
    let error = render("Error");
    let retry = serde_json::to_value(field(&error.entry, "retryPolicyState")).unwrap();
    assert_eq!(retry["kind"], "case");
    assert_eq!(retry["case"], "Counter");
    assert_eq!(retry["payload"]["fields"][0]["name"], "count");
    assert_eq!(retry["payload"]["fields"][0]["value"]["value"], 2);
    let serde_name = |value: serde_json::Value| super::value::text(value.as_str().unwrap());
    assert_eq!(
        field(&error.entry, "kind"),
        &serde_name(
            serde_json::to_value(golem_common::model::oplog::OplogErrorKind::Invocation).unwrap()
        )
    );
    assert_eq!(field(&error.entry, "retryFrom"), &super::value::number(3));
    let log = render("Log");
    assert_eq!(
        field(&log.entry, "level"),
        &serde_name(serde_json::to_value(golem_common::model::oplog::LogLevel::Warn).unwrap())
    );
    let resource = render("CreateResource");
    assert_eq!(
        field(&resource.entry, "name"),
        &super::value::text("resource")
    );
    let finished = render("AgentInvocationFinished");
    assert_eq!(
        field(&finished.entry, "methodName"),
        &super::value::text("do-it")
    );
    assert_eq!(
        field(&finished.entry, "componentRevision"),
        &super::value::number(4)
    );
    let attributed = rendered(KeepFlags::default(), entries.last().unwrap(), None);
    let attribution = as_case(attributed.entry.attribution.as_ref().unwrap());
    assert_eq!(attribution.case.as_deref(), Some("Entity"));
    let call_mode =
        serde_json::to_value(golem_common::model::oplog::PublicEntityCallMode::Synchronous)
            .unwrap();
    assert!(json(&attribution).contains(&call_mode.to_string()));
}
