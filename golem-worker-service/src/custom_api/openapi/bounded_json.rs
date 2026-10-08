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

use super::provider_document::{Category, DocumentError};
use serde::Serialize;
use std::io::{self, Write};

pub(super) const DOCUMENT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

pub(super) fn size(value: &impl Serialize) -> Result<usize, DocumentError> {
    let mut writer = LimitedWriter {
        size: 0,
        inner: io::sink(),
    };
    write_json(value, &mut writer)?;
    Ok(writer.size)
}

pub(super) fn to_vec(value: &impl Serialize) -> Result<Vec<u8>, DocumentError> {
    let mut writer = LimitedWriter {
        size: 0,
        inner: Vec::new(),
    };
    write_json(value, &mut writer)?;
    Ok(writer.inner)
}

fn write_json(value: &impl Serialize, writer: &mut impl Write) -> Result<(), DocumentError> {
    if serde_json::to_writer(writer, value).is_err() {
        return Err(DocumentError::new("generated", Category::MergedSize, ""));
    }
    Ok(())
}

struct LimitedWriter<W> {
    size: usize,
    inner: W,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > DOCUMENT_BYTE_LIMIT - self.size {
            return Err(io::Error::other("merged-size"));
        }
        self.inner.write_all(bytes)?;
        self.size += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn compact_json_limit_counts_utf8_and_encoding_overhead() {
        let exact = "é".repeat((DOCUMENT_BYTE_LIMIT - 2) / 2);
        assert_eq!(size(&exact).unwrap(), DOCUMENT_BYTE_LIMIT);
        assert_eq!(to_vec(&exact).unwrap().len(), DOCUMENT_BYTE_LIMIT);
        assert_eq!(
            to_vec(&(exact + "x")).unwrap_err().category,
            Category::MergedSize
        );
        let escaped = "\n".repeat(DOCUMENT_BYTE_LIMIT / 2);
        assert_eq!(size(&escaped).unwrap_err().category, Category::MergedSize);
    }
}
