test_r::enable!();

use golem_schema::schema::wit::direct::{WireError, decode, encode, encode_async, schema};
use golem_schema::schema::wit::{
    GuestPermissionCardHandle, GuestQuotaTokenHandle, GuestSecretHandle, wire,
};
use golem_schema_derive::{FromWire, IntoWire, WireSchema};
use std::collections::HashMap;
use std::ops::Bound;
use test_r::test;

#[test]
fn snapshot_readers_are_independent_without_clone_or_model_traits() {
    use golem_schema::schema::wit::direct::{FromWire, WireSnapshot};
    let value = Request {
        id: 83,
        values: vec![
            None,
            Some(Err(Fault::Rejected {
                code: 7,
                reason: "denied".into(),
            })),
        ],
    };
    let tree = encode(&value).unwrap();
    let snapshot = std::rc::Rc::new(WireSnapshot::new(tree.value_nodes));
    for _ in 0..2 {
        let mut reader = snapshot.reader();
        assert_eq!(Request::read_wire(&mut reader, tree.root).unwrap(), value);
        reader.finish().unwrap();
    }
    let snapshot = std::rc::Rc::new(WireSnapshot::new(vec![
        wire::SchemaValueNode::U32Value(9),
        wire::SchemaValueNode::TupleValue(vec![0, 0]),
    ]));
    for _ in 0..2 {
        assert!(matches!(
            <(u32, u32)>::read_wire(&mut snapshot.reader(), 1),
            Err(WireError::AliasedNode(0))
        ));
        assert!(matches!(
            u32::read_wire(&mut snapshot.reader(), -1),
            Err(WireError::OutOfBounds(-1))
        ));
    }
}

#[test]
fn shared_readers_keep_resource_reachability_and_discard_checks() {
    use golem_schema::schema::wit::direct::{FromWire, WireSnapshot};
    let snapshot = std::rc::Rc::new(WireSnapshot::new(vec![
        wire::SchemaValueNode::SecretValue(unsafe { wire::Secret::from_handle(61) }),
        wire::SchemaValueNode::U32Value(9),
    ]));
    let mut reader = snapshot.reader();
    assert_eq!(u32::read_wire(&mut reader, 1).unwrap(), 9);
    assert!(matches!(
        reader.finish(),
        Err(WireError::UnreachableResource(0))
    ));
    let mut reader = snapshot.reader();
    reader.discard(0).unwrap();
    reader.finish().unwrap();
    let mut reader = snapshot.reader();
    let handle = GuestSecretHandle::read_wire(&mut reader, 0).unwrap();
    reader.finish().unwrap();
    assert_eq!(handle.take().unwrap().take_handle(), 61);
}

#[test]
fn rich_values_and_nominal_ids_use_direct_wire_shapes() {
    let uuid = uuid::Uuid::from_u64_pair(0x1234, 0x9876);
    let tree = encode(&uuid).unwrap();
    let wire::SchemaValueNode::RecordValue(fields) = &tree.value_nodes[tree.root as usize] else {
        panic!("UUID record")
    };
    assert!(matches!(
        tree.value_nodes[fields[0] as usize],
        wire::SchemaValueNode::U64Value(0x1234)
    ));
    assert!(matches!(
        tree.value_nodes[fields[1] as usize],
        wire::SchemaValueNode::U64Value(0x9876)
    ));
    assert_eq!(decode::<uuid::Uuid>(tree).unwrap(), uuid);
    let promise = golem_schema::PromiseId::new(
        golem_schema::AgentId::new(golem_schema::ComponentId::new(uuid), "Counter(abc)".into()),
        83,
    );
    assert_eq!(
        decode::<golem_schema::PromiseId>(encode(&promise).unwrap()).unwrap(),
        promise
    );
    let mut actual =
        golem_schema::schema::wit::decode_graph(&schema::<golem_schema::PromiseId>()).unwrap();
    let mut expected =
        golem_schema::schema::try_into_schema_graph::<golem_schema::PromiseId>().unwrap();
    actual.defs.sort_by(|a, b| a.id.cmp(&b.id));
    expected.defs.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(actual, expected);
    let date = chrono::DateTime::from_timestamp(-19, 123456789).unwrap();
    assert_eq!(
        decode::<chrono::DateTime<chrono::Utc>>(encode(&date).unwrap()).unwrap(),
        date
    );
    let duration = std::time::Duration::from_nanos(123456789);
    assert_eq!(
        decode::<std::time::Duration>(encode(&duration).unwrap()).unwrap(),
        duration
    );
    assert_eq!(
        decode::<std::time::Duration>(encode(&std::time::Duration::MAX).unwrap()).unwrap(),
        std::time::Duration::from_nanos(i64::MAX as u64)
    );
    assert!(
        decode::<std::time::Duration>(wire::SchemaValueTree {
            root: 0,
            value_nodes: vec![wire::SchemaValueNode::DurationValue(
                wire::DurationValuePayload { nanoseconds: -1 }
            )]
        })
        .is_err()
    );
    assert!(
        decode::<chrono::DateTime<chrono::Utc>>(wire::SchemaValueTree {
            root: 0,
            value_nodes: vec![wire::SchemaValueNode::DatetimeValue(wire::Datetime {
                seconds: i64::MAX,
                nanoseconds: 0
            })]
        })
        .is_err()
    );
}

#[test]
fn standard_maps_and_bounds_roundtrip_directly() {
    let value = HashMap::from([
        ("lower".to_string(), Bound::Included(-4i32)),
        ("upper".to_string(), Bound::Excluded(19i32)),
        ("none".to_string(), Bound::Unbounded),
    ]);
    let encoded = encode(&value).unwrap();
    assert_eq!(
        decode::<HashMap<String, Bound<i32>>>(encoded).unwrap(),
        value
    );

    let graph = schema::<Bound<i32>>();
    let wire::SchemaTypeBody::VariantType(cases) = &graph.type_nodes[graph.root as usize].body
    else {
        panic!("expected bound variant schema");
    };
    assert_eq!(
        cases
            .iter()
            .map(|case| case.name.as_str())
            .collect::<Vec<_>>(),
        ["included", "excluded", "unbounded"]
    );
    assert!(cases[0].payload.is_some());
    assert!(cases[1].payload.is_some());
    assert!(cases[2].payload.is_none());
}

#[cfg(feature = "url")]
#[test]
fn direct_urls_parse_url_nodes_but_not_strings() {
    let url = url::Url::parse("https://example.com/a?b=3").unwrap();
    assert_eq!(decode::<url::Url>(encode(&url).unwrap()).unwrap(), url);
    assert!(decode::<url::Url>(encode("https://example.com/").unwrap()).is_err());
    assert!(
        decode::<url::Url>(wire::SchemaValueTree {
            root: 0,
            value_nodes: vec![wire::SchemaValueNode::UrlValue("not a url".into())]
        })
        .is_err()
    );
}

#[cfg(feature = "bytes")]
#[test]
fn bytes_use_binary_nodes_not_byte_lists() {
    let value = bytes::Bytes::from_static(&[0, 129, 255]);
    let encoded = encode(&value).unwrap();
    assert_eq!(encoded.value_nodes.len(), 1);
    match &encoded.value_nodes[encoded.root as usize] {
        wire::SchemaValueNode::BinaryValue(payload) => {
            assert_eq!(payload.bytes, [0, 129, 255]);
            assert_eq!(payload.mime_type, None);
        }
        _ => panic!("expected binary node"),
    }
    assert_eq!(decode::<bytes::Bytes>(encoded).unwrap(), value);
    let graph = schema::<bytes::Bytes>();
    assert!(matches!(
        graph.type_nodes[graph.root as usize].body,
        wire::SchemaTypeBody::BinaryType(_)
    ));
    assert!(decode::<bytes::Bytes>(encode(&vec![0u8, 129, 255]).unwrap()).is_err());
    assert_eq!(
        decode::<bytes::Bytes>(encode(&bytes::Bytes::new()).unwrap()).unwrap(),
        bytes::Bytes::new()
    );
}

// Deliberately no IntoSchema/FromSchema implementations: a hidden model adapter
// cannot satisfy these tests.
#[derive(Debug, PartialEq, FromWire, IntoWire, WireSchema)]
#[schema(named = "example.Request")]
struct Request {
    #[schema(doc = "request identifier")]
    id: u32,
    values: Vec<Option<Result<String, Fault>>>,
}

#[derive(Debug, PartialEq, FromWire, IntoWire, WireSchema)]
enum Fault {
    Missing,
    Rejected { code: u16, reason: String },
    Retry(u8, bool),
    Nested(Box<Request>),
}

#[test]
fn wire_schema_derive_builds_recursive_flat_arena() {
    let graph = schema::<Request>();
    assert_eq!(graph.defs.len(), 2);
    assert_eq!(graph.defs[0].id, "example.Request");
    assert!(graph.defs.iter().all(|definition| definition.body >= 0));
    assert!(matches!(
        graph.type_nodes[graph.root as usize].body,
        wire::SchemaTypeBody::RefType(0)
    ));

    let request = &graph.type_nodes[graph.defs[0].body as usize];
    let wire::SchemaTypeBody::RecordType(fields) = &request.body else {
        panic!("request must be a record")
    };
    assert_eq!(fields[0].name, "id");
    assert_eq!(
        fields[0].metadata.doc.as_deref(),
        Some("request identifier")
    );

    let fault = graph
        .defs
        .iter()
        .find(|definition| definition.name.as_deref() == Some("Fault"))
        .unwrap();
    assert!(matches!(
        graph.type_nodes[fault.body as usize].body,
        wire::SchemaTypeBody::VariantType(_)
    ));
}

#[test]
fn wire_schema_single_field_variant_payload_matches_encoder() {
    let encoded = encode(&Fault::Nested(Box::new(Request {
        id: 1,
        values: Vec::new(),
    })))
    .unwrap();
    let wire::SchemaValueNode::VariantValue(encoded_variant) =
        &encoded.value_nodes[encoded.root as usize]
    else {
        panic!("fault must encode as a variant")
    };
    assert!(matches!(
        encoded.value_nodes[encoded_variant.payload.unwrap() as usize],
        wire::SchemaValueNode::RecordValue(_)
    ));

    let graph = schema::<Fault>();
    let definition = &graph.defs[0];
    let wire::SchemaTypeBody::VariantType(cases) = &graph.type_nodes[definition.body as usize].body
    else {
        panic!("fault must be a variant")
    };
    let payload = cases[3].payload.expect("tuple case must have a payload");
    assert!(matches!(
        graph.type_nodes[payload as usize].body,
        wire::SchemaTypeBody::RefType(_)
    ));
}

#[derive(Debug, PartialEq, FromWire, IntoWire)]
struct Pair(u16, String);

#[derive(Debug, PartialEq, FromWire, IntoWire)]
struct Empty;

#[derive(Debug, PartialEq, FromWire, IntoWire)]
#[schema(transparent)]
struct Label(String);

fn tree(nodes: Vec<wire::SchemaValueNode>, root: i32) -> wire::SchemaValueTree {
    wire::SchemaValueTree {
        value_nodes: nodes,
        root,
    }
}

#[test]
fn decode_uses_indices_not_arena_order_and_accepts_named_concrete_types() {
    use wire::SchemaValueNode::*;
    let input = tree(
        vec![
            StringValue("denied".into()),
            U32Value(73),
            RecordValue(vec![1, 6]),
            U16Value(409),
            RecordValue(vec![3, 0]),
            VariantValue(wire::VariantValuePayload {
                case: 1,
                payload: Some(4),
            }),
            ListValue(vec![8, 9]),
            ResultValue(wire::ResultValuePayload::ErrValue(Some(5))),
            OptionValue(Some(7)),
            OptionValue(None),
        ],
        2,
    );
    assert_eq!(
        decode::<Request>(input).unwrap(),
        Request {
            id: 73,
            values: vec![
                Some(Err(Fault::Rejected {
                    code: 409,
                    reason: "denied".into()
                })),
                None
            ],
        }
    );
}

#[test]
fn encoder_emits_exact_wire_shapes_without_schema_traits() {
    use wire::SchemaValueNode::*;
    let encoded = encode(&Request {
        id: 91,
        values: vec![Some(Ok("ok".into()))],
    })
    .unwrap();
    assert_eq!(encoded.root, 5);
    assert_eq!(encoded.value_nodes.len(), 6);
    assert!(matches!(&encoded.value_nodes[0], U32Value(91)));
    assert!(matches!(&encoded.value_nodes[1], StringValue(s) if s == "ok"));
    assert!(matches!(
        &encoded.value_nodes[2],
        ResultValue(wire::ResultValuePayload::OkValue(Some(1)))
    ));
    assert!(matches!(&encoded.value_nodes[3], OptionValue(Some(2))));
    assert!(matches!(&encoded.value_nodes[4], ListValue(indices) if indices == &[3]));
    assert!(matches!(&encoded.value_nodes[5], RecordValue(indices) if indices == &[0, 4]));
}

#[test]
fn structural_errors_are_rejected_before_entering_concrete_body() {
    use wire::SchemaValueNode::*;
    assert_eq!(
        decode::<Pair>(tree(vec![TupleValue(vec![1]), U16Value(4)], 0)),
        Err(WireError::Shape("field count"))
    );
    assert_eq!(
        decode::<Pair>(tree(
            vec![
                TupleValue(vec![1, 2, 3]),
                U16Value(4),
                StringValue("x".into()),
                BoolValue(false)
            ],
            0
        )),
        Err(WireError::Shape("field count"))
    );
    assert_eq!(
        decode::<Pair>(tree(
            vec![TupleValue(vec![2, 1]), U16Value(4), StringValue("x".into())],
            0
        )),
        Err(WireError::Shape("U16Value"))
    );
    assert_eq!(
        decode::<Vec<u16>>(tree(vec![ListValue(vec![1, 1]), U16Value(4)], 0)),
        Err(WireError::AliasedNode(1))
    );
    assert_eq!(
        decode::<Option<Box<Option<u16>>>>(tree(vec![OptionValue(Some(0))], 0)),
        Err(WireError::AliasedNode(0))
    );
    assert_eq!(
        decode::<String>(tree(vec![StringValue("x".into())], -1)),
        Err(WireError::OutOfBounds(-1))
    );
    assert_eq!(
        decode::<String>(tree(vec![], 0)),
        Err(WireError::OutOfBounds(0))
    );
    assert_eq!(
        decode::<Fault>(tree(
            vec![VariantValue(wire::VariantValuePayload {
                case: 2,
                payload: None
            })],
            0
        )),
        Err(WireError::Shape("variant payload"))
    );
    assert_eq!(
        decode::<Fault>(tree(
            vec![
                VariantValue(wire::VariantValuePayload {
                    case: 0,
                    payload: Some(1)
                }),
                U8Value(2)
            ],
            0
        )),
        Err(WireError::Shape("absent variant payload"))
    );
    assert_eq!(
        decode::<Fault>(tree(
            vec![VariantValue(wire::VariantValuePayload {
                case: 99,
                payload: None
            })],
            0
        )),
        Err(WireError::Shape("variant case"))
    );
}

#[test]
fn result_unit_payload_and_unit_record_are_distinct() {
    use wire::SchemaValueNode::*;
    let value = encode(&Ok::<(), String>(())).unwrap();
    assert_eq!(value.value_nodes.len(), 1);
    assert!(matches!(
        value.value_nodes[0],
        ResultValue(wire::ResultValuePayload::OkValue(None))
    ));
    assert_eq!(decode::<Result<(), String>>(value).unwrap(), Ok(()));
    assert_eq!(
        decode::<Result<String, ()>>(tree(
            vec![ResultValue(wire::ResultValuePayload::OkValue(None))],
            0
        )),
        Err(WireError::Shape("result payload"))
    );
    assert_eq!(
        decode::<Empty>(tree(vec![RecordValue(vec![])], 0)),
        Ok(Empty)
    );
    assert_eq!(
        decode::<Empty>(tree(vec![TupleValue(vec![])], 0)),
        Err(WireError::Shape("RecordValue"))
    );
    assert_eq!(
        decode::<Label>(tree(vec![StringValue("label".into())], 0)),
        Ok(Label("label".into()))
    );
}

#[test]
fn unit_result_rejects_present_payload() {
    use wire::SchemaValueNode::*;
    assert!(
        decode::<Result<(), String>>(tree(
            vec![
                ResultValue(wire::ResultValuePayload::OkValue(Some(1))),
                TupleValue(vec![]),
            ],
            0,
        ))
        .is_err()
    );
}

#[test]
fn nested_custom_errors_roundtrip() {
    for value in [
        Fault::Missing,
        Fault::Retry(9, false),
        Fault::Nested(Box::new(Request {
            id: 6,
            values: vec![None, Some(Ok("deep".into()))],
        })),
    ] {
        assert_eq!(decode::<Fault>(encode(&value).unwrap()).unwrap(), value);
    }
}

#[derive(FromWire, IntoWire)]
struct Resources {
    secret: GuestSecretHandle,
    tokens: Vec<Option<GuestQuotaTokenHandle>>,
    card: GuestPermissionCardHandle,
    stream: golem_schema::SchemaValueStream,
}

#[test]
async fn direct_resource_transfer_consumes_handles_once() {
    let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(17) });
    let quota = GuestQuotaTokenHandle::new(unsafe { wire::QuotaToken::from_handle(29) });
    let card = GuestPermissionCardHandle::new(unsafe { wire::PermissionCard::from_handle(43) });
    let stream = golem_schema::SchemaValueStream::from_wrapped(unsafe {
        wire::SchemaValueStream::from_handle(59)
    });
    let value = Resources {
        secret: secret.clone(),
        tokens: vec![None, Some(quota.clone())],
        card: card.clone(),
        stream: stream.clone(),
    };
    let encoded = encode_async(&value).await.unwrap();
    assert!(!secret.is_present());
    assert!(!quota.is_present());
    assert!(!card.is_present());
    assert!(!stream.is_present());
    use golem_schema::schema::wit::direct::{FromWire, WireSnapshot};
    let snapshot = std::rc::Rc::new(WireSnapshot::new(encoded.value_nodes));
    let mut first = snapshot.reader();
    let decoded = Resources::read_wire(&mut first, encoded.root).unwrap();
    first.finish().unwrap();
    let mut second = snapshot.reader();
    let alias = Resources::read_wire(&mut second, encoded.root).unwrap();
    second.finish().unwrap();
    assert_eq!(decoded.secret.cell_id(), alias.secret.cell_id());
    assert_eq!(decoded.card.cell_id(), alias.card.cell_id());
    assert_eq!(
        decoded.tokens[1].as_ref().unwrap().cell_id(),
        alias.tokens[1].as_ref().unwrap().cell_id()
    );
    let forwarded = encode(&decoded).unwrap();
    let mut handles = Vec::new();
    for node in forwarded.value_nodes {
        match node {
            wire::SchemaValueNode::SecretValue(handle) => {
                handles.push(("secret", handle.take_handle()))
            }
            wire::SchemaValueNode::QuotaTokenHandle(handle) => {
                handles.push(("quota", handle.take_handle()))
            }
            wire::SchemaValueNode::PermissionCardHandle(handle) => {
                handles.push(("card", handle.take_handle()))
            }
            wire::SchemaValueNode::StreamValue(handle) => {
                handles.push(("stream", handle.take_handle()))
            }
            _ => {}
        }
    }
    assert_eq!(
        handles,
        vec![("secret", 17), ("quota", 29), ("card", 43), ("stream", 59)]
    );
    assert!(matches!(
        encode(&alias),
        Err(WireError::ConsumedResource("secret"))
    ));
}

#[test]
fn resource_preflight_failure_preserves_earlier_and_aliased_handles() {
    let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(71) });
    let quota = GuestQuotaTokenHandle::new(unsafe { wire::QuotaToken::from_handle(83) });
    let value = (secret.clone(), vec![quota.clone(), quota.clone()]);
    assert!(matches!(
        encode(&value),
        Err(WireError::AliasedResource("quota-token"))
    ));
    assert!(secret.is_present());
    assert!(quota.is_present());
    assert_eq!(quota.take().unwrap().take_handle(), 83);
    assert!(matches!(
        encode(&value),
        Err(WireError::ConsumedResource("quota-token"))
    ));
    assert!(secret.is_present());
    assert_eq!(secret.take().unwrap().take_handle(), 71);
}

#[test]
fn constructor_quota_rejection_preserves_every_handle() {
    use golem_schema::schema::wit::direct::{IntoWire, WirePreflight};

    let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(71) });
    let quota = GuestQuotaTokenHandle::new(unsafe { wire::QuotaToken::from_handle(83) });
    let value = (secret.clone(), vec![None, Some(quota.clone())]);
    let mut preflight = WirePreflight::default();
    value.preflight(&mut preflight).unwrap();
    assert!(matches!(
        preflight.reject_quota_tokens(),
        Err(WireError::ForbiddenResource("quota-token"))
    ));
    assert_eq!(secret.take().unwrap().take_handle(), 71);
    assert_eq!(quota.take().unwrap().take_handle(), 83);
    let mut preflight = WirePreflight::default();
    ("ordinary".to_string(), vec![1u32, 7])
        .preflight(&mut preflight)
        .unwrap();
    preflight.reject_quota_tokens().unwrap();
}

#[test]
fn stream_presence_uses_type_metadata_without_building_a_schema() {
    use golem_schema::schema::wit::direct::{WireSchema, WireSchemaBuilder};
    use std::collections::{BTreeMap, HashSet};

    struct Stream;
    impl WireSchema for Stream {
        fn contains_stream(_: &mut HashSet<&'static str>) -> bool {
            true
        }
        fn append_schema(_: &mut WireSchemaBuilder) -> i32 {
            panic!("must not build a schema")
        }
    }
    #[allow(dead_code)]
    #[derive(WireSchema)]
    struct Recursive {
        children: Vec<Recursive>,
    }
    #[allow(dead_code)]
    #[derive(WireSchema)]
    struct RecursiveStream {
        children: Vec<RecursiveStream>,
        stream: Option<Stream>,
    }
    assert!(!Recursive::contains_stream(&mut HashSet::new()));
    assert!(RecursiveStream::contains_stream(&mut HashSet::new()));
    assert!(
        <BTreeMap<String, Result<Box<RecursiveStream>, u32>>>::contains_stream(&mut HashSet::new())
    );
    assert!(!<(String, Option<Result<Box<Recursive>, u32>>)>::contains_stream(&mut HashSet::new()));
}

#[derive(Debug, PartialEq, IntoWire, FromWire, WireSchema)]
struct Rich {
    #[schema(text(language = "hu", regex = "[a-z]+"))]
    text: String,
    #[schema(binary(mime_type = "application/octet-stream"))]
    bytes: Vec<u8>,
    #[schema(url(allowed_schemes = "https"))]
    url: String,
    #[schema(skip)]
    ignored: bool,
}

#[test]
fn rich_fields_preserve_wire_semantics_without_guest_validation() {
    use wire::SchemaValueNode::*;
    let graph = schema::<Rich>();
    let wire::SchemaTypeBody::RecordType(fields) =
        &graph.type_nodes[graph.defs[0].body as usize].body
    else {
        panic!("expected rich record schema");
    };
    assert_eq!(fields.len(), 3);
    assert!(
        matches!(&graph.type_nodes[fields[0].body as usize].body, wire::SchemaTypeBody::TextType(spec) if spec.languages.as_deref() == Some(&["hu".to_string()]) && spec.regex.as_deref() == Some("[a-z]+"))
    );
    assert!(
        matches!(&graph.type_nodes[fields[1].body as usize].body, wire::SchemaTypeBody::BinaryType(spec) if spec.mime_types.as_deref() == Some(&["application/octet-stream".to_string()]))
    );
    assert!(
        matches!(&graph.type_nodes[fields[2].body as usize].body, wire::SchemaTypeBody::UrlType(spec) if spec.allowed_schemes.as_deref() == Some(&["https".to_string()]))
    );
    let value = Rich {
        text: "ÁRVÍZ".into(),
        bytes: vec![0, 255, 13],
        url: "host-validation-is-not-this-converters-job".into(),
        ignored: false,
    };
    let encoded = encode(&value).unwrap();
    assert!(
        matches!(&encoded.value_nodes[0], TextValue(v) if v.text == "ÁRVÍZ" && v.language.as_deref() == Some("hu"))
    );
    assert!(
        matches!(&encoded.value_nodes[1], BinaryValue(v) if v.bytes == [0, 255, 13] && v.mime_type.as_deref() == Some("application/octet-stream"))
    );
    assert!(matches!(&encoded.value_nodes[2], UrlValue(v) if v == &value.url));
    assert!(matches!(&encoded.value_nodes[3], RecordValue(v) if v == &[0, 1, 2]));
    assert_eq!(decode::<Rich>(encoded).unwrap(), value);
}

#[derive(Debug, PartialEq, IntoWire, FromWire, WireSchema)]
#[schema(union, rename_all = "kebab-case")]
enum Choice<T> {
    #[schema(prefix = "x")]
    FirstValue(T),
    #[schema(rename = "other", suffix = "y")]
    SecondValue(T),
}

#[derive(Debug, PartialEq, IntoWire, FromWire, WireSchema)]
#[schema(transparent)]
struct Unit(());

#[test]
fn boxed_transparent_unit_result_schema_has_no_payload() {
    let graph = schema::<Result<Box<Unit>, String>>();
    let wire::SchemaTypeBody::ResultType(spec) = &graph.type_nodes[graph.root as usize].body else {
        panic!("expected result schema");
    };
    assert!(spec.ok.is_none());
    assert!(matches!(
        graph.type_nodes[spec.err.unwrap() as usize].body,
        wire::SchemaTypeBody::StringType
    ));
    let encoded = encode(&Ok::<Box<Unit>, String>(Box::new(Unit(())))).unwrap();
    assert!(matches!(
        encoded.value_nodes[encoded.root as usize],
        wire::SchemaValueNode::ResultValue(wire::ResultValuePayload::OkValue(None))
    ));
}

#[derive(Debug, PartialEq, IntoWire, FromWire)]
enum Mode {
    First,
    Second,
}

#[test]
fn generic_union_tags_unit_payloads_and_enum_indices_are_direct() {
    use wire::SchemaValueNode::*;
    let graph = schema::<Choice<String>>();
    let wire::SchemaTypeBody::UnionType(spec) = &graph.type_nodes[graph.defs[0].body as usize].body
    else {
        panic!("expected union schema");
    };
    assert_eq!(spec.branches[0].tag, "first-value");
    assert_eq!(spec.branches[1].tag, "other");
    assert!(
        matches!(&spec.branches[1].discriminator, wire::DiscriminatorRule::Suffix(suffix) if suffix == "y")
    );
    assert!(matches!(
        graph.type_nodes[spec.branches[1].body as usize].body,
        wire::SchemaTypeBody::StringType
    ));
    let encoded = encode(&Choice::SecondValue("body".to_string())).unwrap();
    assert!(matches!(&encoded.value_nodes[1], UnionValue(v) if v.tag == "other" && v.body == 0));
    assert_eq!(
        decode::<Choice<String>>(encoded).unwrap(),
        Choice::SecondValue("body".into())
    );
    assert!(
        decode::<Choice<String>>(tree(
            vec![
                UnionValue(wire::UnionValuePayload {
                    tag: "SecondValue".into(),
                    body: 1
                }),
                StringValue("body".into())
            ],
            0
        ))
        .is_err()
    );
    let encoded = encode(&Err::<String, Unit>(Unit(()))).unwrap();
    assert!(matches!(
        &encoded.value_nodes[0],
        ResultValue(wire::ResultValuePayload::ErrValue(None))
    ));
    assert_eq!(
        decode::<Result<String, Unit>>(encoded).unwrap(),
        Err(Unit(()))
    );
    assert_eq!(
        decode::<Mode>(tree(vec![EnumValue(1)], 0)).unwrap(),
        Mode::Second
    );
    assert!(decode::<Mode>(tree(vec![EnumValue(2)], 0)).is_err());
}

#[allow(dead_code)]
mod shadowed_names {
    use golem_schema_derive::{FromWire, IntoWire};

    struct Result;
    struct Option;
    struct Ok;
    struct Err;
    struct Some;
    struct None;
    struct Default;

    #[derive(Debug, PartialEq, FromWire, IntoWire)]
    pub struct Message {
        #[schema(text(language = "en"))]
        pub text: String,
        #[schema(skip)]
        pub ignored: bool,
    }

    #[derive(Debug, PartialEq, FromWire, IntoWire)]
    pub enum Never {}
}

#[test]
fn derives_handle_shadowed_prelude_names_and_uninhabited_variants() {
    let value = shadowed_names::Message {
        text: "message".into(),
        ignored: false,
    };
    assert_eq!(
        decode::<shadowed_names::Message>(encode(&value).unwrap()).unwrap(),
        value
    );
    assert_eq!(
        decode::<shadowed_names::Never>(tree(
            vec![wire::SchemaValueNode::VariantValue(
                wire::VariantValuePayload {
                    case: 0,
                    payload: None,
                }
            )],
            0,
        )),
        Err(WireError::Shape("variant case"))
    );
}

#[test]
fn skipped_generic_field_does_not_require_wire_traits() {
    struct NotWire;

    #[derive(IntoWire, FromWire, WireSchema)]
    struct Envelope<T> {
        value: u8,
        #[schema(skip)]
        marker: std::marker::PhantomData<T>,
    }

    let value = Envelope::<NotWire> {
        value: 7,
        marker: std::marker::PhantomData,
    };
    let encoded = encode(&value).unwrap();
    assert_eq!(decode::<Envelope<NotWire>>(encoded).unwrap().value, 7);
    let graph = schema::<Envelope<NotWire>>();
    let wire::SchemaTypeBody::RecordType(fields) =
        &graph.type_nodes[graph.defs[0].body as usize].body
    else {
        panic!("expected record schema");
    };
    assert_eq!(fields.len(), 1);

    #[derive(IntoWire, FromWire)]
    struct WithDefault<T> {
        value: u16,
        #[schema(skip)]
        extra: T,
    }

    #[derive(Default)]
    struct LocalOnly;
    let encoded = encode(&WithDefault {
        value: 8193,
        extra: LocalOnly,
    })
    .unwrap();
    let decoded = decode::<WithDefault<LocalOnly>>(encoded).unwrap();
    assert_eq!(decoded.value, 8193);
    let _: LocalOnly = decoded.extra;
}
