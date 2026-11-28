test_r::enable!();

use golem_schema::schema::wit::{
    DecodeError, EncodeError, GuestQuotaTokenHandle, GuestSecretHandle, decode_typed_owned,
    encode_graph, encode_typed_owned, wire,
};
use golem_schema::schema::{SchemaGraph, SchemaValue, TypedSchemaValue};
use test_r::test;

#[test]
fn json_contract_and_named_roles_round_trip() {
    use golem_schema::schema::wit::{decode_graph, decode_metadata, encode_metadata};
    use golem_schema::schema::{MetadataEnvelope, Role, SchemaType};

    let graph = SchemaGraph::anonymous(SchemaType::json());
    assert_eq!(decode_graph(&encode_graph(&graph).unwrap()).unwrap(), graph);
    let value = TypedSchemaValue::new(
        graph,
        SchemaValue::Json(r#"{ "b": [1,null,{"x":"y"}], "a": 1.5 }"#.into()),
    );
    let encoded = encode_typed_owned(value.clone()).unwrap();
    assert!(matches!(
        encoded.value.value_nodes[encoded.value.root as usize],
        wire::SchemaValueNode::JsonValue(_)
    ));
    assert_eq!(decode_typed_owned(encoded).unwrap(), value);
    for inner in [None, Some(Box::new(SchemaValue::Json("null".into())))] {
        let value = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::option(SchemaType::json())),
            SchemaValue::Option { inner },
        );
        assert_eq!(
            decode_typed_owned(encode_typed_owned(value.clone()).unwrap()).unwrap(),
            value
        );
    }
    for role in [
        Role::StandardMultimodal,
        Role::TaskUpdate,
        Role::Other("StandardMultimodal".into()),
        Role::Other("standard-multimodal".into()),
        Role::Other("task-update".into()),
    ] {
        let metadata = MetadataEnvelope {
            role: Some(role),
            ..Default::default()
        };
        assert_eq!(decode_metadata(&encode_metadata(&metadata)), metadata);
    }
}

fn typed(value: SchemaValue) -> TypedSchemaValue {
    TypedSchemaValue::new(SchemaGraph::empty(), value)
}

fn release_wire_handles(value: &wire::TypedSchemaValue) -> (Vec<u32>, Vec<u32>) {
    let mut secrets = Vec::new();
    let mut quotas = Vec::new();
    for node in &value.value.value_nodes {
        match node {
            wire::SchemaValueNode::SecretValue(handle) => {
                secrets.push(handle.take_handle());
            }
            wire::SchemaValueNode::QuotaTokenHandle(handle) => {
                quotas.push(handle.take_handle());
            }
            _ => {}
        }
    }
    (secrets, quotas)
}

#[test]
fn owned_typed_values_forward_nested_secret_and_quota_handles_once() {
    let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(11) });
    let quota = GuestQuotaTokenHandle::new(unsafe { wire::QuotaToken::from_handle(12) });
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::List {
                elements: vec![SchemaValue::Secret(secret.clone())],
            },
            SchemaValue::Tuple {
                elements: vec![SchemaValue::QuotaToken(quota.clone())],
            },
        ],
    };

    let wire_value = encode_typed_owned(typed(value)).expect("initial transfer succeeds");
    let decoded = decode_typed_owned(wire_value).expect("owned decode succeeds");
    let forwarded = encode_typed_owned(decoded).expect("forwarding transfer succeeds");
    let (secrets, quotas) = release_wire_handles(&forwarded);

    assert_eq!(secrets, vec![11]);
    assert_eq!(quotas, vec![12]);
    assert!(!secret.is_present());
    assert!(!quota.is_present());
}

#[test]
fn owned_typed_encoding_rejects_duplicate_secret_references_before_transfer() {
    let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(21) });
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::Secret(secret.clone()),
            SchemaValue::List {
                elements: vec![SchemaValue::Secret(secret.clone())],
            },
        ],
    };

    let error = encode_typed_owned(typed(value)).expect_err("aliasing must be rejected");
    assert_eq!(error, EncodeError::AliasedSecretHandle);
    assert!(secret.is_present());

    let handle = secret
        .take()
        .expect("failed preflight preserves the handle");
    assert_eq!(handle.take_handle(), 21);
}

#[test]
fn owned_typed_encoding_rejects_a_second_quota_transfer() {
    let quota = GuestQuotaTokenHandle::new(unsafe { wire::QuotaToken::from_handle(31) });
    let first = encode_typed_owned(typed(SchemaValue::QuotaToken(quota.clone())))
        .expect("first transfer succeeds");
    let (_, quotas) = release_wire_handles(&first);
    assert_eq!(quotas, vec![31]);

    let error = encode_typed_owned(typed(SchemaValue::QuotaToken(quota)))
        .expect_err("second transfer must fail");
    assert_eq!(error, EncodeError::QuotaTokenAlreadyConsumed);
}

#[test]
fn owned_typed_decoding_rejects_aliased_wire_nodes() {
    let value = wire::TypedSchemaValue {
        graph: encode_graph(&SchemaGraph::empty()).expect("test graph encodes"),
        value: wire::SchemaValueTree {
            value_nodes: vec![
                wire::SchemaValueNode::RecordValue(vec![1, 1]),
                wire::SchemaValueNode::StringValue("shared".to_string()),
            ],
            root: 0,
        },
    };

    let error = decode_typed_owned(value).expect_err("aliased nodes must be rejected");
    assert_eq!(error, DecodeError::AliasedValueNode(1));
}
