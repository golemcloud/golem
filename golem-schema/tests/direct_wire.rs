test_r::enable!();

use golem_schema::schema::wit::direct::{WireError, decode, encode, encode_async};
use golem_schema::schema::wit::{
    GuestPermissionCardHandle, GuestQuotaTokenHandle, GuestSecretHandle, wire,
};
use golem_schema_derive::{FromWire, IntoWire};
use test_r::test;

// Deliberately no IntoSchema/FromSchema implementations: a hidden model adapter
// cannot satisfy these tests.
#[derive(Debug, PartialEq, FromWire, IntoWire)]
#[schema(named = "example.Request")]
struct Request {
    id: u32,
    values: Vec<Option<Result<String, Fault>>>,
}

#[derive(Debug, PartialEq, FromWire, IntoWire)]
enum Fault {
    Missing,
    Rejected { code: u16, reason: String },
    Retry(u8, bool),
    Nested(Box<Request>),
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
    let decoded = decode::<Resources>(encoded).unwrap();
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
        encode(&decoded),
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

#[derive(Debug, PartialEq, IntoWire, FromWire)]
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

#[derive(Debug, PartialEq, IntoWire, FromWire)]
#[schema(union, rename_all = "kebab-case")]
enum Choice<T> {
    #[schema(prefix = "x")]
    FirstValue(T),
    #[schema(rename = "other", suffix = "y")]
    SecondValue(T),
}

#[derive(Debug, PartialEq, IntoWire, FromWire)]
#[schema(transparent)]
struct Unit(());

#[derive(Debug, PartialEq, IntoWire, FromWire)]
enum Mode {
    First,
    Second,
}

#[test]
fn generic_union_tags_unit_payloads_and_enum_indices_are_direct() {
    use wire::SchemaValueNode::*;
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

    #[derive(IntoWire, FromWire)]
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
