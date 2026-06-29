test_r::enable!();

use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, Quantity, QuantityUnit, Schema, SchemaValue,
    SecretRef, decode_typed_schema_value, encode_typed_schema_value, schema::try_into_schema_graph,
};
use test_r::test;

#[derive(Clone, Debug, PartialEq, IntoSchema, FromSchema)]
struct ReexportedSchemaType {
    name: String,
    count: u32,
}

#[derive(Clone, Debug, PartialEq, Schema)]
struct ConvenienceSchemaType {
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

#[test]
fn derive_schema_convenience_through_golem_rust_reexport() {
    let graph = try_into_schema_graph::<ConvenienceSchemaType>().expect("schema graph is valid");
    assert_eq!(graph.defs.len(), 1);

    let value = ConvenienceSchemaType {
        name: "test".to_string(),
        count: 42,
    };
    let encoded = value.to_value();
    let decoded = ConvenienceSchemaType::from_value(&encoded).expect("value decodes");
    assert_eq!(decoded, value);
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
    let encoded = value.to_value();
    assert!(
        matches!(encoded, SchemaValue::Url { .. }),
        "url::Url must encode to the rich Url schema value, got {encoded:?}"
    );
    assert_eq!(url::Url::from_value(&encoded).unwrap(), value);
}

#[test]
fn path_schema_round_trip() {
    let value = std::path::PathBuf::from("/tmp/golem/α.txt");
    let encoded = value.to_value();
    assert!(
        matches!(encoded, SchemaValue::Path { .. }),
        "PathBuf must encode to the rich Path schema value, got {encoded:?}"
    );
    assert_eq!(std::path::PathBuf::from_value(&encoded).unwrap(), value);
}

struct TestBytes;

impl QuantityUnit for TestBytes {
    fn type_id() -> golem_rust::schema::TypeId {
        golem_rust::schema::TypeId::new("golem.it.test.TestBytes")
    }
    fn base_unit() -> &'static str {
        "B"
    }
}

#[test]
fn quantity_schema_round_trip() {
    let value = Quantity::<TestBytes>::new(123, 1, "B").unwrap();
    let encoded = value.to_value();
    assert!(
        matches!(encoded, SchemaValue::Quantity(_)),
        "Quantity must encode to the rich Quantity schema value, got {encoded:?}"
    );
    assert_eq!(Quantity::<TestBytes>::from_value(&encoded).unwrap(), value);
}

#[test]
fn quantity_rejects_disallowed_unit() {
    assert!(Quantity::<TestBytes>::new(1, 0, "kg").is_err());
}

#[test]
fn secret_ref_schema_round_trip() {
    let value = SecretRef::new("secret-ref-abc").unwrap();
    let encoded = value.to_value();
    assert!(
        matches!(encoded, SchemaValue::Secret(_)),
        "SecretRef must encode to the rich Secret schema value, got {encoded:?}"
    );
    assert_eq!(SecretRef::from_value(&encoded).unwrap(), value);
}
