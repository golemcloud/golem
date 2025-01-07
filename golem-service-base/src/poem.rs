// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
};

use poem::web::Field as PoemField;
use tempfile::NamedTempFile;

use poem_openapi::{
    registry::{MetaSchema, MetaSchemaRef},
    types::{ParseError, ParseFromMultipartField, ParseResult, Type},
};

/// A uploaded file for multipart.
///
/// Similiar to https://github.com/poem-web/poem/blob/f7bb838253fdf0bf67d3592fce45b92d68242c97/poem-openapi/src/types/multipart/upload.rs#L25,
/// but also gives you access to the underlying file.
/// This is useful if you need multiple independent reads from the file.
pub struct TempFileUpload {
    file_name: Option<String>,
    content_type: Option<String>,
    file: NamedTempFile,
}

impl Debug for TempFileUpload {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Upload");
        if let Some(file_name) = self.file_name() {
            d.field("filename", &file_name);
        }
        if let Some(content_type) = self.content_type() {
            d.field("content_type", &content_type);
        }
        d.finish()
    }
}

impl TempFileUpload {
    /// Get the content type of the field.
    #[inline]
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The file name found in the `Content-Disposition` header.
    #[inline]
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Consumes this body object to return the file.
    pub fn into_file(self) -> NamedTempFile {
        self.file
    }
}

impl Type for TempFileUpload {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string(binary)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "binary")))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ParseFromMultipartField for TempFileUpload {
    async fn parse_from_multipart(field: Option<PoemField>) -> ParseResult<Self> {
        match field {
            Some(field) => {
                let content_type = field.content_type().map(ToString::to_string);
                let file_name = field.file_name().map(ToString::to_string);
                let file = field_to_tempfile(field).await.map_err(ParseError::custom)?;
                Ok(Self {
                    content_type,
                    file_name,
                    file,
                })
            }
            None => Err(ParseError::expected_input()),
        }
    }
}

async fn field_to_tempfile(field: PoemField) -> Result<NamedTempFile, std::io::Error> {
    let mut reader = field.into_async_read();

    let file = tempfile::NamedTempFile::new()?;

    let mut tokio_file = tokio::fs::File::from_std(file.reopen()?);
    tokio::io::copy(&mut reader, &mut tokio_file).await?;

    Ok(file)
}
