//! MCP content projection into typed values and optional stdout bytes.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use golem_schema::schema::unstructured::{
    unstructured_binary_schema_type, unstructured_inline_value,
};
use golem_schema::schema::{
    BinaryRestrictions, BinaryValuePayload, MetadataEnvelope, NamedFieldType, SchemaType,
    SchemaValue, VariantCaseType, VariantValuePayload,
};
use serde_json::{Map, Value};
use thiserror::Error;

fn consumed_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "text" => &["type", "text", "annotations"],
        "image" | "audio" => &["type", "data", "mimeType", "annotations"],
        "resource_link" => &[
            "type",
            "uri",
            "name",
            "title",
            "description",
            "mimeType",
            "size",
            "annotations",
        ],
        "resource" => &["type", "resource", "annotations"],
        _ => &[],
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProjectionLimits {
    pub max_blocks: usize,
    pub max_total_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_json_depth: usize,
}

impl Default for ProjectionLimits {
    fn default() -> Self {
        Self {
            max_blocks: 256,
            max_total_bytes: 8 * 1024 * 1024,
            max_decoded_bytes: 4 * 1024 * 1024,
            max_json_depth: 32,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ProjectedContent {
    pub value: SchemaValue,
    pub stdout: Option<Vec<u8>>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ContentError {
    #[error("content has {actual} blocks, exceeding limit {limit}")]
    BlockLimit { actual: usize, limit: usize },
    #[error("content exceeds total byte limit {0}")]
    TotalBytes(usize),
    #[error("decoded content exceeds byte limit {0}")]
    DecodedBytes(usize),
    #[error("content JSON exceeds nesting depth limit {0}")]
    JsonDepth(usize),
    #[error("content block {index}: {message}")]
    Malformed { index: usize, message: String },
}

/// The stable carried-tag schema: `none | streamed | blocks`.
pub fn schema() -> SchemaType {
    variant(vec![
        case("none", None),
        case(
            "streamed",
            Some(record(vec![
                field("mime-type", SchemaType::string()),
                field("annotations", option_string()),
                field("extensions", option_string()),
            ])),
        ),
        case("blocks", Some(SchemaType::list(block_schema()))),
    ])
}

pub fn project(
    content: &[Value],
    limits: ProjectionLimits,
) -> Result<ProjectedContent, ContentError> {
    check_limits(content, limits)?;
    let mut decoded_total = 0;
    if content.is_empty() {
        return Ok(ProjectedContent {
            value: tagged(0, None),
            stdout: None,
        });
    }
    if content.len() == 1 {
        let obj = object(&content[0], 0)?;
        let kind = string(obj, "type", 0)?;
        if kind == "text" {
            let text = string(obj, "text", 0)?;
            charge_decoded(text.len(), limits, &mut decoded_total)?;
            return Ok(streamed(
                obj,
                "text/plain; charset=utf-8",
                text.as_bytes().to_vec(),
            ));
        }
        if kind == "image" || kind == "audio" {
            let mime = string(obj, "mimeType", 0)?;
            let bytes = decode(string(obj, "data", 0)?, limits, 0, &mut decoded_total)?;
            return Ok(streamed(obj, mime, bytes));
        }
    }
    let elements = content
        .iter()
        .enumerate()
        .map(|(i, value)| block(value, i, limits, &mut decoded_total))
        .collect::<Result<_, _>>()?;
    Ok(ProjectedContent {
        value: tagged(2, Some(SchemaValue::List { elements })),
        stdout: None,
    })
}

/// Render bounded MCP error content independently of successful-content validation.
pub fn error_payload(content: &[Value], limits: ProjectionLimits) -> Result<String, ContentError> {
    check_limits(content, limits)?;
    if let [Value::Object(obj)] = content
        && obj.get("type").and_then(Value::as_str) == Some("text")
        && let Some(text) = obj.get("text").and_then(Value::as_str)
    {
        charge_decoded(text.len(), limits, &mut 0)?;
        return Ok(text.to_owned());
    }
    crate::limits::check_bytes(&content, limits.max_decoded_bytes)
        .map_err(|_| ContentError::DecodedBytes(limits.max_decoded_bytes))?;
    Ok(serde_json::to_string(content).expect("JSON values always serialize"))
}

fn streamed(obj: &Map<String, Value>, mime: &str, bytes: Vec<u8>) -> ProjectedContent {
    ProjectedContent {
        value: tagged(
            1,
            Some(record_value(vec![
                SchemaValue::String(mime.into()),
                opt_json(obj.get("annotations")),
                extensions(obj, consumed_fields(obj["type"].as_str().unwrap())),
            ])),
        ),
        stdout: Some(bytes),
    }
}

fn block(
    value: &Value,
    index: usize,
    limits: ProjectionLimits,
    decoded_total: &mut usize,
) -> Result<SchemaValue, ContentError> {
    let obj = object(value, index)?;
    let annotations = opt_json(obj.get("annotations"));
    let kind = string(obj, "type", index)?;
    let ext = extensions(obj, consumed_fields(kind));
    let result = match kind {
        "text" => {
            let text = string(obj, "text", index)?;
            charge_decoded(text.len(), limits, decoded_total)?;
            tagged(
                0,
                Some(record_value(vec![
                    SchemaValue::String(text.into()),
                    annotations,
                    ext,
                ])),
            )
        }
        "image" | "audio" => {
            let kind = string(obj, "type", index)?;
            let bytes = decode(string(obj, "data", index)?, limits, index, decoded_total)?;
            let payload = unstructured_inline_value(SchemaValue::Binary(BinaryValuePayload {
                bytes,
                mime_type: Some(string(obj, "mimeType", index)?.into()),
            }));
            tagged(
                if kind == "image" { 1 } else { 2 },
                Some(record_value(vec![payload, annotations, ext])),
            )
        }
        "resource_link" => tagged(
            3,
            Some(record_value(vec![
                str_value(obj, "uri", index)?,
                str_value(obj, "name", index)?,
                opt_str(obj, "title", index)?,
                opt_str(obj, "description", index)?,
                opt_str(obj, "mimeType", index)?,
                opt_u64(obj, "size", index)?,
                annotations,
                ext,
            ])),
        ),
        "resource" => embedded(obj, annotations, ext, index, limits, decoded_total)?,
        other => {
            return Err(malformed(
                index,
                format!("unsupported content type `{other}`"),
            ));
        }
    };
    Ok(result)
}

fn embedded(
    obj: &Map<String, Value>,
    annotations: SchemaValue,
    ext: SchemaValue,
    index: usize,
    limits: ProjectionLimits,
    decoded_total: &mut usize,
) -> Result<SchemaValue, ContentError> {
    let resource = obj
        .get("resource")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed(index, "missing or non-object `resource`"))?;
    let uri = str_value(resource, "uri", index)?;
    let mime = opt_str(resource, "mimeType", index)?;
    let resource_ext = extensions(resource, &["uri", "mimeType", "text", "blob", "_meta"]);
    let body = match (resource.get("text"), resource.get("blob")) {
        (Some(Value::String(text)), None) => {
            charge_decoded(text.len(), limits, decoded_total)?;
            tagged(0, Some(SchemaValue::String(text.clone())))
        }
        (None, Some(Value::String(blob))) => tagged(
            1,
            Some(unstructured_inline_value(SchemaValue::Binary(
                BinaryValuePayload {
                    bytes: decode(blob, limits, index, decoded_total)?,
                    mime_type: resource
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                },
            ))),
        ),
        _ => {
            return Err(malformed(
                index,
                "embedded resource must contain exactly one string `text` or `blob`",
            ));
        }
    };
    Ok(tagged(
        4,
        Some(record_value(vec![
            uri,
            mime,
            body,
            annotations,
            ext,
            resource_ext,
        ])),
    ))
}

fn block_schema() -> SchemaType {
    let meta = || {
        vec![
            field("annotations", option_string()),
            field("extensions", option_string()),
        ]
    };
    let mut text = vec![field("text", SchemaType::string())];
    text.extend(meta());
    let binary = || {
        let mut f = vec![field(
            "data",
            unstructured_binary_schema_type(BinaryRestrictions::default()),
        )];
        f.extend(meta());
        f
    };
    variant(vec![
        case("text", Some(record(text))),
        case("image", Some(record(binary()))),
        case("audio", Some(record(binary()))),
        case(
            "resource-link",
            Some(record(vec![
                field("uri", SchemaType::string()),
                field("name", SchemaType::string()),
                field("title", option_string()),
                field("description", option_string()),
                field("mime-type", option_string()),
                field("size", SchemaType::option(SchemaType::u64())),
                field("annotations", option_string()),
                field("extensions", option_string()),
            ])),
        ),
        case(
            "embedded-resource",
            Some(record(vec![
                field("uri", SchemaType::string()),
                field("mime-type", option_string()),
                field(
                    "body",
                    variant(vec![
                        case("text", Some(SchemaType::string())),
                        case(
                            "blob",
                            Some(unstructured_binary_schema_type(
                                BinaryRestrictions::default(),
                            )),
                        ),
                    ]),
                ),
                field("annotations", option_string()),
                field("extensions", option_string()),
                field("resource-extensions", option_string()),
            ])),
        ),
    ])
}

fn check_limits(content: &[Value], limits: ProjectionLimits) -> Result<(), ContentError> {
    if content.len() > limits.max_blocks {
        return Err(ContentError::BlockLimit {
            actual: content.len(),
            limit: limits.max_blocks,
        });
    }
    let mut nodes = limits.max_total_bytes;
    for value in content {
        crate::limits::walk(value, limits.max_json_depth, &mut nodes).map_err(|error| {
            if error == crate::limits::Exceeded::Depth {
                ContentError::JsonDepth(limits.max_json_depth)
            } else {
                ContentError::TotalBytes(limits.max_total_bytes)
            }
        })?;
    }
    crate::limits::check_bytes(&content, limits.max_total_bytes)
        .map_err(|_| ContentError::TotalBytes(limits.max_total_bytes))
}

fn decode(
    value: &str,
    limits: ProjectionLimits,
    index: usize,
    decoded_total: &mut usize,
) -> Result<Vec<u8>, ContentError> {
    let estimate = value.len().saturating_add(3) / 4 * 3;
    if decoded_total.saturating_add(estimate) > limits.max_decoded_bytes.saturating_add(2) {
        return Err(ContentError::DecodedBytes(limits.max_decoded_bytes));
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|e| malformed(index, format!("invalid standard base64: {e}")))?;
    charge_decoded(bytes.len(), limits, decoded_total)?;
    Ok(bytes)
}

fn charge_decoded(
    bytes: usize,
    limits: ProjectionLimits,
    total: &mut usize,
) -> Result<(), ContentError> {
    *total = total.saturating_add(bytes);
    if *total > limits.max_decoded_bytes {
        return Err(ContentError::DecodedBytes(limits.max_decoded_bytes));
    }
    Ok(())
}

fn object(value: &Value, index: usize) -> Result<&Map<String, Value>, ContentError> {
    value
        .as_object()
        .ok_or_else(|| malformed(index, "block must be an object"))
}
fn string<'a>(
    obj: &'a Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<&'a str, ContentError> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(index, format!("missing or non-string `{key}`")))
}
fn str_value(
    obj: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<SchemaValue, ContentError> {
    Ok(SchemaValue::String(string(obj, key, index)?.into()))
}
fn opt_str(obj: &Map<String, Value>, key: &str, index: usize) -> Result<SchemaValue, ContentError> {
    match obj.get(key) {
        None => Ok(opt(None)),
        Some(Value::String(v)) => Ok(opt(Some(SchemaValue::String(v.clone())))),
        _ => Err(malformed(index, format!("non-string `{key}`"))),
    }
}
fn opt_u64(obj: &Map<String, Value>, key: &str, index: usize) -> Result<SchemaValue, ContentError> {
    match obj.get(key) {
        None => Ok(opt(None)),
        Some(v) => v
            .as_u64()
            .map(|n| opt(Some(SchemaValue::U64(n))))
            .ok_or_else(|| malformed(index, format!("non-u64 `{key}`"))),
    }
}
fn opt_json(value: Option<&Value>) -> SchemaValue {
    opt(value.map(|v| SchemaValue::String(v.to_string())))
}
fn extensions(obj: &Map<String, Value>, known: &[&str]) -> SchemaValue {
    let mut out = Map::new();
    if let Some(v) = obj.get("_meta") {
        out.insert("_meta".into(), v.clone());
    }
    for (k, v) in obj {
        if !known.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    if out.is_empty() {
        opt(None)
    } else {
        opt(Some(SchemaValue::String(Value::Object(out).to_string())))
    }
}
fn opt(v: Option<SchemaValue>) -> SchemaValue {
    SchemaValue::Option {
        inner: v.map(Box::new),
    }
}
fn tagged(case: u32, payload: Option<SchemaValue>) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case,
        payload: payload.map(Box::new),
    })
}
fn record_value(fields: Vec<SchemaValue>) -> SchemaValue {
    SchemaValue::Record { fields }
}
fn malformed(index: usize, message: impl Into<String>) -> ContentError {
    ContentError::Malformed {
        index,
        message: message.into(),
    }
}
fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.into(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}
fn case(name: &str, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.into(),
        payload,
        metadata: MetadataEnvelope::default(),
    }
}
fn record(fields: Vec<NamedFieldType>) -> SchemaType {
    SchemaType::record(fields)
}
fn variant(cases: Vec<VariantCaseType>) -> SchemaType {
    SchemaType::variant(cases)
}
fn option_string() -> SchemaType {
    SchemaType::option(SchemaType::string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_schema::schema::{SchemaGraph, validation::validate_value};
    use serde_json::json;
    use test_r::test;

    fn p(v: Vec<Value>) -> Result<ProjectedContent, ContentError> {
        project(&v, ProjectionLimits::default())
    }
    fn valid(p: &ProjectedContent) {
        validate_value(&SchemaGraph::anonymous(schema()), &schema(), &p.value).unwrap();
    }

    #[test]
    fn empty_and_simple_text() {
        let x = p(vec![]).unwrap();
        valid(&x);
        assert_eq!(x.value, tagged(0, None));
        let x = p(vec![json!({"type":"text","text":""})]).unwrap();
        valid(&x);
        assert_eq!(x.stdout, Some(vec![]));
        let x = p(vec![json!({"type":"text","text":"hé🦀"})]).unwrap();
        assert_eq!(x.stdout, Some("hé🦀".as_bytes().to_vec()));
    }
    #[test]
    fn binary_streams_and_preserves_metadata() {
        for kind in ["image", "audio"] {
            let x=p(vec![json!({"type":kind,"data":"AAEC/w==","mimeType":format!("{kind}/x-test"),"annotations":{"priority":0.5},"vendor":7})]).unwrap();
            valid(&x);
            assert_eq!(x.stdout, Some(vec![0, 1, 2, 255]));
            let s = serde_json::to_string(&x.value).unwrap();
            assert!(s.contains("priority") && s.contains("vendor"));
        }
    }
    #[test]
    fn multiple_blocks_keep_order_and_boundaries() {
        let x = p(vec![
            json!({"type":"text","text":"a"}),
            json!({"type":"text","text":"b"}),
        ])
        .unwrap();
        valid(&x);
        assert_eq!(x.stdout, None);
        let SchemaValue::Variant(v) = x.value else {
            panic!()
        };
        let SchemaValue::List { elements } = *v.payload.unwrap() else {
            panic!()
        };
        assert_eq!(elements.len(), 2);
        for (element, expected) in elements.iter().zip(["a", "b"]) {
            let SchemaValue::Variant(value) = element else {
                panic!()
            };
            assert_eq!(value.case, 0);
            let SchemaValue::Record { fields } = value.payload.as_deref().unwrap() else {
                panic!()
            };
            assert_eq!(fields[0], SchemaValue::String(expected.into()));
        }
    }
    #[test]
    fn mixed_and_resources_are_typed() {
        let values = vec![
            json!({"type":"text","text":"x"}),
            json!({"type":"image","data":"AQI=","mimeType":"image/png"}),
        ];
        let x = p(values).unwrap();
        valid(&x);
        assert!(x.stdout.is_none());
        let link=p(vec![json!({"type":"resource_link","uri":"https://x","name":"n","title":"t","description":"d","mimeType":"text/x","size":2,"annotations":{"audience":["user"]},"x":1})]).unwrap();
        valid(&link);
        assert!(link.stdout.is_none());
    }
    #[test]
    fn embedded_text_and_blob() {
        for resource in [
            json!({"uri":"u","mimeType":"text/plain","text":"body","etag":"e"}),
            json!({"uri":"u","mimeType":"application/x","blob":"AP8=","etag":"e"}),
        ] {
            let x=p(vec![json!({"type":"resource","resource":resource,"annotations":{"lastModified":"now"},"_meta":{"id":1}})]).unwrap();
            valid(&x);
            let s = serde_json::to_string(&x.value).unwrap();
            assert!(s.contains("etag") && s.contains("lastModified") && s.contains("id"));
        }
    }

    #[test]
    fn mixed_binary_fields_use_inline_sdk_carriers_without_losing_bytes_or_mime() {
        use golem_schema::schema::unstructured::{
            UnstructuredValueCase, decode_unstructured_value,
        };
        let projected = p(vec![
            json!({"type":"image","data":"AP8=","mimeType":"image/png"}),
            json!({"type":"audio","data":"AQI=","mimeType":"audio/wav"}),
            json!({"type":"resource","resource":{"uri":"u","blob":"Aw=="}}),
        ])
        .unwrap();
        valid(&projected);
        assert!(projected.stdout.is_none());
        let SchemaValue::Variant(content) = projected.value else {
            panic!()
        };
        let SchemaValue::List { elements } = *content.payload.unwrap() else {
            panic!()
        };
        for (block, (case, expected_bytes, expected_mime)) in elements.iter().zip([
            (1, vec![0, 255], Some("image/png")),
            (2, vec![1, 2], Some("audio/wav")),
            (4, vec![3], None),
        ]) {
            let SchemaValue::Variant(block) = block else {
                panic!()
            };
            assert_eq!(block.case, case);
            let SchemaValue::Record { fields } = block.payload.as_deref().unwrap() else {
                panic!()
            };
            let carrier = if case == 4 {
                let SchemaValue::Variant(body) = &fields[2] else {
                    panic!()
                };
                assert_eq!(body.case, 1);
                body.payload.as_deref().unwrap()
            } else {
                &fields[0]
            };
            let UnstructuredValueCase::Inline(SchemaValue::Binary(binary)) =
                decode_unstructured_value(carrier).unwrap()
            else {
                panic!()
            };
            assert_eq!(binary.bytes, expected_bytes);
            assert_eq!(binary.mime_type.as_deref(), expected_mime);
        }
    }

    #[test]
    fn malformed_inputs() {
        assert!(
            p(vec![json!({"type":"image","data":"%%%","mimeType":"x/y"})])
                .unwrap_err()
                .to_string()
                .contains("base64")
        );
        assert!(p(vec![json!({"type":"text"})]).is_err());
        assert!(
            p(vec![json!({"type":"video"})])
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(
            p(vec![
                json!({"type":"resource","resource":{"uri":"u","text":"x","blob":"eA=="}})
            ])
            .is_err()
        );
    }
    #[test]
    fn limits_apply_on_both_sides() {
        let content = [json!({"type":"text","text":"\""})];
        let exact = r#"[{"text":"\"","type":"text"}]"#.len();
        let limits = ProjectionLimits {
            max_total_bytes: exact,
            ..Default::default()
        };
        assert_eq!(project(&content, limits).unwrap().stdout, Some(vec![b'"']));
        assert!(matches!(
            project(
                &content,
                ProjectionLimits {
                    max_total_bytes: exact - 1,
                    ..limits
                }
            ),
            Err(ContentError::TotalBytes(_))
        ));
        let content = [
            json!({"type":"text","text":"x"}),
            json!({"type":"image","data":"AAEC/w==","mimeType":"image/png"}),
        ];
        let limits = ProjectionLimits {
            max_blocks: 2,
            max_decoded_bytes: 5,
            ..Default::default()
        };
        assert!(project(&content, limits).unwrap().stdout.is_none());
        assert!(matches!(
            project(
                &content,
                ProjectionLimits {
                    max_decoded_bytes: 4,
                    ..limits
                }
            ),
            Err(ContentError::DecodedBytes(_))
        ));
        assert!(matches!(
            project(
                &content,
                ProjectionLimits {
                    max_blocks: 1,
                    ..limits
                }
            ),
            Err(ContentError::BlockLimit { .. })
        ));
        let mut l = ProjectionLimits {
            max_blocks: 1,
            ..Default::default()
        };
        assert!(matches!(
            project(&[json!({}), json!({})], l),
            Err(ContentError::BlockLimit { .. })
        ));
        l = ProjectionLimits {
            max_total_bytes: 2,
            ..Default::default()
        };
        assert!(matches!(
            project(&[json!({"type":"text","text":"long"})], l),
            Err(ContentError::TotalBytes(_))
        ));
        l = ProjectionLimits {
            max_decoded_bytes: 2,
            ..Default::default()
        };
        assert!(matches!(
            project(&[json!({"type":"image","data":"AQID","mimeType":"x/y"})], l),
            Err(ContentError::DecodedBytes(_))
        ));
        l = ProjectionLimits {
            max_json_depth: 1,
            ..Default::default()
        };
        assert!(matches!(
            project(&[json!({"x":{"y":1}})], l),
            Err(ContentError::JsonDepth(_))
        ));
    }
    #[test]
    fn embedded_metadata_namespaces_do_not_overwrite_each_other() {
        let value = p(vec![json!({"type":"resource","_meta":{"id":"outer"},"resource":{"uri":"u","text":"body","_meta":{"id":"inner"}}})]).unwrap();
        let SchemaValue::Variant(value) = value.value else {
            panic!()
        };
        let SchemaValue::List { elements } = value.payload.as_deref().unwrap() else {
            panic!()
        };
        let SchemaValue::Variant(value) = &elements[0] else {
            panic!()
        };
        let SchemaValue::Record { fields } = value.payload.as_deref().unwrap() else {
            panic!()
        };
        for (index, id) in [(4, "outer"), (5, "inner")] {
            let SchemaValue::Option { inner: Some(value) } = &fields[index] else {
                panic!()
            };
            let SchemaValue::String(json) = value.as_ref() else {
                panic!()
            };
            assert_eq!(
                serde_json::from_str::<Value>(json).unwrap(),
                json!({"_meta":{"id":id}})
            );
        }
    }
    #[test]
    fn errors_render_without_success_validation() {
        assert_eq!(
            error_payload(
                &[json!({"type":"text","text":"unchanged"})],
                ProjectionLimits::default()
            )
            .unwrap(),
            "unchanged"
        );
        assert_eq!(
            error_payload(&[], ProjectionLimits::default()).unwrap(),
            "[]"
        );
        assert_eq!(
            error_payload(
                &[json!({"type":"unknown","x":1})],
                ProjectionLimits::default()
            )
            .unwrap(),
            r#"[{"type":"unknown","x":1}]"#
        );
    }

    #[test]
    fn errors_are_bounded_without_requiring_successful_block_shapes() {
        let content = [json!({"type":"text","text":"hé"})];
        let limits = ProjectionLimits {
            max_decoded_bytes: 3,
            ..Default::default()
        };
        assert_eq!(error_payload(&content, limits).unwrap(), "hé");
        assert!(matches!(
            error_payload(
                &content,
                ProjectionLimits {
                    max_decoded_bytes: 2,
                    ..limits
                }
            ),
            Err(ContentError::DecodedBytes(_))
        ));
        let unknown = [json!({"vendor":true})];
        let size = r#"[{"vendor":true}]"#.len();
        let limits = ProjectionLimits {
            max_decoded_bytes: size,
            max_total_bytes: size,
            max_json_depth: 1,
            max_blocks: 1,
        };
        assert_eq!(
            error_payload(&unknown, limits).unwrap(),
            r#"[{"vendor":true}]"#
        );
        assert!(matches!(
            error_payload(
                &unknown,
                ProjectionLimits {
                    max_total_bytes: size - 1,
                    ..limits
                }
            ),
            Err(ContentError::TotalBytes(_))
        ));
        assert!(matches!(
            error_payload(
                &unknown,
                ProjectionLimits {
                    max_decoded_bytes: size - 1,
                    ..limits
                }
            ),
            Err(ContentError::DecodedBytes(_))
        ));
        assert!(matches!(
            error_payload(
                &unknown,
                ProjectionLimits {
                    max_json_depth: 0,
                    ..limits
                }
            ),
            Err(ContentError::JsonDepth(_))
        ));
        assert!(matches!(
            error_payload(
                &unknown,
                ProjectionLimits {
                    max_blocks: 0,
                    ..limits
                }
            ),
            Err(ContentError::BlockLimit { .. })
        ));
    }
}
