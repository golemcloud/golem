test_r::enable!();

use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, SchemaValue, decode_typed_schema_value,
    encode_typed_schema_value, schema::try_into_schema_graph,
};
use test_r::test;

#[derive(Clone, Debug, PartialEq, IntoSchema, FromSchema)]
struct ReexportedSchemaType {
    name: String,
    count: u32,
}

#[test]
fn derive_schema_through_golem_rust_reexports() {
    let graph = try_into_schema_graph::<ReexportedSchemaType>().expect("schema graph is valid");
    assert_eq!(graph.defs.len(), 1);

    let value = ReexportedSchemaType {
        name: "test".to_string(),
        count: 42,
    };
    let encoded = value.to_value();
    assert!(matches!(encoded, SchemaValue::Record { .. }));

    let decoded = ReexportedSchemaType::from_value(&encoded).expect("value decodes");
    assert_eq!(decoded, value);

    let typed = value
        .into_typed_schema_value()
        .expect("typed schema value is valid");
    let wire = encode_typed_schema_value(&typed).expect("typed value encodes to WIT");
    let decoded_wire = decode_typed_schema_value(&wire).expect("typed value decodes from WIT");
    assert_eq!(decoded_wire, typed);
}

#[cfg(feature = "bytes")]
#[test]
fn bytes_schema_round_trip() {
    let value = bytes::Bytes::from(vec![1, 2, 3]);
    assert_eq!(bytes::Bytes::from_value(&value.to_value()).unwrap(), value);
}

#[cfg(feature = "mac_address")]
#[test]
fn mac_address_schema_round_trip() {
    let value = mac_address::MacAddress::new([1, 2, 3, 4, 5, 6]);
    assert_eq!(
        mac_address::MacAddress::from_value(&value.to_value()).unwrap(),
        value
    );
}

#[cfg(feature = "num_bigint")]
#[test]
fn bigint_schema_round_trip() {
    let value = num_bigint::BigInt::from(-1234567890i64);
    assert_eq!(
        num_bigint::BigInt::from_value(&value.to_value()).unwrap(),
        value
    );
}

#[cfg(feature = "rust_decimal")]
#[test]
fn decimal_schema_round_trip() {
    let value = rust_decimal::Decimal::new(-12345, 2);
    assert_eq!(
        rust_decimal::Decimal::from_value(&value.to_value()).unwrap(),
        value
    );
}

#[cfg(feature = "nonempty_collections")]
#[test]
fn nonempty_vec_schema_round_trip() {
    let value = nonempty_collections::NEVec::try_from_vec(vec![1u32, 2, 3]).unwrap();
    assert_eq!(
        nonempty_collections::NEVec::<u32>::from_value(&value.to_value()).unwrap(),
        value
    );
}

#[cfg(feature = "url")]
#[test]
fn url_schema_round_trip() {
    let value = url::Url::parse("https://example.com/path").unwrap();
    assert_eq!(url::Url::from_value(&value.to_value()).unwrap(), value);
}
