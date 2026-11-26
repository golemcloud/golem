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

///|
/// Ergonomic wrapper for the role-marked unstructured-text schema type
/// (`variant { inline: text<restrictions>, url: url }`). A value is either
/// inline text (with an optional language code) or a URL reference.
pub(all) enum UnstructuredText {
  TextInline(String, String?)
  TextUrl(String)
} derive(Debug, Eq)

///|
pub extend UnstructuredText with Debug::{to_repr}

///|
pub extend UnstructuredText with Eq::{equal, not_equal}

///|
/// Ergonomic wrapper for the role-marked unstructured-binary schema type
/// (`variant { inline: binary<restrictions>, url: url }`). A value is either
/// inline bytes (with an optional MIME type) or a URL reference.
pub(all) enum UnstructuredBinary {
  BinaryInline(Bytes, String?)
  BinaryUrl(String)
} derive(Debug, Eq)

///|
pub extend UnstructuredBinary with Debug::{to_repr}

///|
pub extend UnstructuredBinary with Eq::{equal, not_equal}

// Variant case indices of the canonical role-marked unstructured wrappers:
// `variant { inline, url }`.

///|
pub fn unstructured_text_to_schema_value(
  input : UnstructuredText,
) -> SchemaValue {
  match input {
    TextInline(value, language) =>
      VariantValue(0, Some(TextValue(value, language)))
    TextUrl(url) => VariantValue(1, Some(UrlValue(url)))
  }
}

///|
pub fn unstructured_text_from_schema_value(
  parameter_name : String,
  value : SchemaValue,
) -> UnstructuredText raise {
  match value {
    VariantValue(1, Some(UrlValue(url))) => TextUrl(url)
    VariantValue(0, Some(TextValue(text, language))) => TextInline(text, language)
    other =>
      raise BridgeError(
        "Invalid value for parameter \{parameter_name}. Expected an unstructured-text variant, got \{@debug.to_string(other)}",
      )
  }
}

///|
pub fn unstructured_binary_to_schema_value(
  input : UnstructuredBinary,
) -> SchemaValue {
  match input {
    BinaryInline(bytes, mime_type) =>
      VariantValue(0, Some(BinaryValue(bytes, mime_type)))
    BinaryUrl(url) => VariantValue(1, Some(UrlValue(url)))
  }
}

///|
pub fn unstructured_binary_from_schema_value(
  parameter_name : String,
  value : SchemaValue,
) -> UnstructuredBinary raise {
  match value {
    VariantValue(1, Some(UrlValue(url))) => BinaryUrl(url)
    VariantValue(0, Some(BinaryValue(bytes, mime_type))) =>
      BinaryInline(bytes, mime_type)
    other =>
      raise BridgeError(
        "Invalid value for parameter \{parameter_name}. Expected an unstructured-binary variant, got \{@debug.to_string(other)}",
      )
  }
}
