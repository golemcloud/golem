use base64::Engine;
use golem_common::base_model::agent::{
    BinaryReference, BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema,
    ElementSchema, TextReference, TextReferenceValue, TextSource, TextType, UntypedElementValue,
};
use golem_wasm::json::ValueAndTypeJsonExtensions;

pub fn extract_multimodal_element_value(
    name: &str,
    value_json: &serde_json::Value,
    elem_schema: &ElementSchema,
    index: usize,
) -> Result<UntypedElementValue, String> {
    match elem_schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            let value_and_type =
                golem_wasm::ValueAndType::parse_with_type(value_json, element_type).map_err(
                    |errs| {
                        format!(
                            "parts[{}] '{}': failed to parse value: {}",
                            index,
                            name,
                            errs.join(", ")
                        )
                    },
                )?;
            Ok(UntypedElementValue::ComponentModel(value_and_type.value))
        }
        ElementSchema::UnstructuredText(descriptor) => {
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and optional 'languageCode'",
                    index, name
                )
            })?;

            let data = obj
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("parts[{}] '{}': missing 'data' string field", index, name))?
                .to_string();

            let language_code = obj.get("languageCode").and_then(|v| v.as_str());

            if let Some(code) = language_code
                && let Some(allowed) = &descriptor.restrictions
                && !allowed.is_empty()
                && !allowed.iter().any(|t| t.language_code == code)
            {
                let expected: Vec<&str> =
                    allowed.iter().map(|t| t.language_code.as_str()).collect();
                return Err(format!(
                    "parts[{}] '{}': language code '{}' is not allowed. Expected one of: {}",
                    index,
                    name,
                    code,
                    expected.join(", ")
                ));
            }

            let text_type = language_code.map(|code| TextType {
                language_code: code.to_string(),
            });

            Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                value: TextReference::Inline(TextSource { data, text_type }),
            }))
        }
        ElementSchema::UnstructuredBinary(descriptor) => {
            let obj = value_json.as_object().ok_or_else(|| {
                format!(
                    "parts[{}] '{}': value must be an object with 'data' and 'mimeType'",
                    index, name
                )
            })?;

            let b64 = obj.get("data").and_then(|v| v.as_str()).ok_or_else(|| {
                format!("parts[{}] '{}': missing 'data' string field", index, name)
            })?;

            let mime_type = obj
                .get("mimeType")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!(
                        "parts[{}] '{}': missing 'mimeType' string field",
                        index, name
                    )
                })?;

            if let Some(allowed) = &descriptor.restrictions
                && !allowed.is_empty()
                && !allowed.iter().any(|t| t.mime_type == mime_type)
            {
                let expected: Vec<&str> = allowed.iter().map(|t| t.mime_type.as_str()).collect();
                return Err(format!(
                    "parts[{}] '{}': MIME type '{}' is not allowed. Expected one of: {}",
                    index,
                    name,
                    mime_type,
                    expected.join(", ")
                ));
            }

            let data = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    format!(
                        "parts[{}] '{}': failed to decode base64: {}",
                        index, name, e
                    )
                })?;

            Ok(UntypedElementValue::UnstructuredBinary(
                BinaryReferenceValue {
                    value: BinaryReference::Inline(BinarySource {
                        data,
                        binary_type: BinaryType {
                            mime_type: mime_type.to_string(),
                        },
                    }),
                },
            ))
        }
    }
}
