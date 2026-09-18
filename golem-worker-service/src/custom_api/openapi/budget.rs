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
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(super) const GENERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const DOCUMENT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Budget {
    pub deadline: Instant,
    pub cancelled: CancellationToken,
}

impl Budget {
    pub fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            cancelled: CancellationToken::new(),
        }
    }

    pub fn check(&self) -> Result<(), DocumentError> {
        if self.cancelled.is_cancelled() || Instant::now() > self.deadline {
            Err(DocumentError::new("generated", Category::Timeout, ""))
        } else {
            Ok(())
        }
    }

    pub fn size(&self, value: &impl Serialize) -> Result<usize, DocumentError> {
        let mut writer = LimitedWriter {
            budget: self,
            size: 0,
            inner: io::sink(),
        };
        self.write_json(value, &mut writer)?;
        Ok(writer.size)
    }

    pub fn json(&self, value: &impl Serialize) -> Result<Vec<u8>, DocumentError> {
        let mut writer = LimitedWriter {
            budget: self,
            size: 0,
            inner: Vec::new(),
        };
        self.write_json(value, &mut writer)?;
        Ok(writer.inner)
    }

    fn write_json(
        &self,
        value: &impl Serialize,
        writer: &mut impl Write,
    ) -> Result<(), DocumentError> {
        self.check()?;
        if serde_json::to_writer(writer, value).is_err() {
            self.check()?;
            return Err(DocumentError::new("generated", Category::MergedSize, ""));
        }
        self.check()
    }
}

struct LimitedWriter<'a, W> {
    budget: &'a Budget,
    size: usize,
    inner: W,
}

impl<W: Write> Write for LimitedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.budget
            .check()
            .map_err(|_| io::Error::other("generation-timeout"))?;
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
        let budget = Budget::new(Instant::now() + GENERATION_TIMEOUT);
        let exact = "é".repeat((DOCUMENT_BYTE_LIMIT - 2) / 2);
        assert_eq!(budget.size(&exact).unwrap(), DOCUMENT_BYTE_LIMIT);
        assert_eq!(budget.json(&exact).unwrap().len(), DOCUMENT_BYTE_LIMIT);
        assert_eq!(
            budget.json(&(exact + "x")).unwrap_err().category,
            Category::MergedSize
        );
        let escaped = "\n".repeat(DOCUMENT_BYTE_LIMIT / 2);
        assert_eq!(
            budget.size(&escaped).unwrap_err().category,
            Category::MergedSize
        );
    }

    #[test]
    fn deadline_is_inclusive_and_cancellation_interrupts_serialization() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::pause();
                let budget = Budget::new(Instant::now());
                assert!(budget.json(&serde_json::json!({})).is_ok());
                tokio::time::advance(Duration::from_nanos(1)).await;
                assert_eq!(
                    budget.json(&serde_json::json!({})).unwrap_err().category,
                    Category::Timeout
                );
                let budget = Budget::new(Instant::now() + GENERATION_TIMEOUT);
                budget.cancelled.cancel();
                assert_eq!(
                    budget.json(&serde_json::json!({})).unwrap_err().category,
                    Category::Timeout
                );
            });
    }
}
