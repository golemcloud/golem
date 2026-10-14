test_r::enable!();

use golem_rust::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, Quantity, QuantityUnit, Schema, SchemaValue,
    decode_typed_schema_value, encode_typed_schema_value, schema::try_into_schema_graph,
    schema_fingerprint_v1,
};
use test_r::test;

// Every golem:permissions function is available through the typed permissions API.
#[test]
fn every_permission_operation_is_exposed_through_the_typed_api() {
    use golem_rust::permissions::{
        self, Bounds, CardId, CardView, Holder, KernelModule, Lifespan, PatternGrant,
        PermissionCard, PermissionError, ScopeCard, ScopeParent, Timestamp, WalletVersionToken,
    };

    let _: fn(&PermissionCard) -> CardId = PermissionCard::id;
    let _: fn(&PermissionCard) -> Vec<CardId> = PermissionCard::parents;
    let _: fn(&PermissionCard) -> Option<Timestamp> = PermissionCard::expires_at;
    let _: fn(&PermissionCard) -> Result<CardView, PermissionError> = PermissionCard::inspect;
    let _: fn(
        &PermissionCard,
        &Bounds,
        Option<&Lifespan>,
    ) -> Result<PermissionCard, PermissionError> = PermissionCard::derive;
    let _: fn(PermissionCard) -> Result<u32, PermissionError> = PermissionCard::revoke;
    let _: fn(PermissionCard, &Holder) -> Result<(), PermissionError> = PermissionCard::install;
    let _: fn() -> Vec<PermissionCard> = permissions::wallet;
    let _: fn() -> WalletVersionToken = permissions::wallet_version;
    let _: fn(&Bounds, Option<&Lifespan>) -> Result<PermissionCard, PermissionError> =
        permissions::derive_from_wallet;
    let _: fn(&[ScopeParent<'_>], &Bounds) -> Result<ScopeCard, PermissionError> =
        permissions::derive_scope;
    let _: fn(&ScopeCard) -> CardId = ScopeCard::id;
    let _: fn(&ScopeCard) -> Vec<CardId> = ScopeCard::parents;
    let _: fn(&ScopeCard) -> Option<Timestamp> = ScopeCard::expires_at;
    let _: fn(&ScopeCard) -> Result<CardView, PermissionError> = ScopeCard::inspect;
    let _: fn() -> Vec<KernelModule> = permissions::list_modules;
    let _: fn(&PatternGrant) -> Result<(), PermissionError> = permissions::validate_grant;
}

#[derive(Clone, Debug, PartialEq, IntoSchema, FromSchema)]
struct ReexportedSchemaType {
    name: String,
    count: u32,
}

#[derive(Clone, Debug, PartialEq, Schema)]
struct ConvenienceSchemaType {
    name: String,
    count: u32,
    choice: ConvenienceChoice<String>,
}

#[derive(Clone, Debug, PartialEq, Schema)]
#[schema(named = "test.ConvenienceChoice")]
enum ConvenienceChoice<T> {
    Empty,
    Value(T),
    Retry { delay: u32, reason: Option<T> },
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
    let mut wire_graph =
        golem_rust::decode_schema_graph(&golem_rust::schema::wit::direct::schema::<
            ConvenienceSchemaType,
        >())
        .unwrap();
    let mut graph = graph;
    graph.defs.sort_by(|a, b| a.id.cmp(&b.id));
    wire_graph.defs.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(wire_graph, graph);

    for choice in [
        ConvenienceChoice::Empty,
        ConvenienceChoice::Value("selected".into()),
        ConvenienceChoice::Retry {
            delay: 19,
            reason: None,
        },
        ConvenienceChoice::Retry {
            delay: 7,
            reason: Some("busy".into()),
        },
    ] {
        let value = ConvenienceSchemaType {
            name: "test".to_string(),
            count: 42,
            choice,
        };
        let encoded = value.to_value();
        let decoded = ConvenienceSchemaType::from_value(&encoded).expect("value decodes");
        assert_eq!(decoded, value);
        let wire = golem_rust::schema::wit::direct::encode(&value).unwrap();
        assert_eq!(golem_rust::decode_schema_value(wire).unwrap(), encoded);
        let wire = golem_rust::schema::wit::direct::encode(&value).unwrap();
        assert_eq!(
            golem_rust::schema::wit::direct::decode::<ConvenienceSchemaType>(wire).unwrap(),
            value
        );
    }
}

#[test]
fn schema_fingerprint_golden_vectors_through_golem_rust_reexport() {
    let graph = golem_rust::SchemaGraph::empty();
    assert_eq!(
        schema_fingerprint_v1(&graph, None).unwrap().to_hex(),
        "b50494cf0f33961c703d5f6e6af3d3159e528c4d09c1d801172cdf8f022dcafa"
    );
    assert_eq!(
        schema_fingerprint_v1(&graph, Some(&golem_rust::SchemaType::string()))
            .unwrap()
            .to_hex(),
        "61c50c0a3c6ffd63529621ada78afc0d4d8e5fe691f8b0993035f847c660a307"
    );
    assert_eq!(
        schema_fingerprint_v1(
            &graph,
            Some(&golem_rust::SchemaType::list(
                golem_rust::SchemaType::string()
            ))
        )
        .unwrap()
        .to_hex(),
        "4939707f8ef97e9d4b31b568332eaf5a3011f2be7c358f7546966fadfb9416d4"
    );
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
