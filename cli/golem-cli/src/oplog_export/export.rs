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

//! The export: drives the obfuscator over the oplog reads of one agent.
//!
//! The export pins the root agent by the `instance_id` of its `Create` entry, renders the oplog
//! page by page and checks the root again at the end, so a report never mixes two incarnations of
//! an agent. Each page is written to the sink before the next read.

use crate::oplog_export::model::{
    KEEP_SCALARS_WITHOUT_SHAPE_NOTE, OmittedEntryRef, OmittedReason, OplogExportCoverage,
    OplogExportHeader, ReportAgentRef, ReportEntry, StartUnknownRef,
};
use crate::oplog_export::obfuscator::{
    KeepFlags, ObfuscationContext, TokenKey, agent_reference, render_entry, render_omitted,
};
use async_trait::async_trait;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    OmittedOplogEntry, OplogCursor, PublicOplogEntry, PublicOplogEntryWithIndex,
    PublicOplogPageEntry,
};
use std::collections::HashMap;
use uuid::Uuid;

/// One page of an oplog read.
#[derive(Debug, Clone)]
pub struct OplogPage {
    pub entries: Vec<PublicOplogPageEntry>,
    /// The cursor of the next page; `None` after the last page.
    pub next: Option<OplogCursor>,
}

/// The two oplog reads of the export.
#[async_trait]
pub trait OplogReads: Send + Sync {
    /// Reads one page. A read with a query never sends `from`.
    async fn read_page(
        &self,
        from: Option<u64>,
        count: u64,
        cursor: Option<&OplogCursor>,
        query: Option<&str>,
    ) -> anyhow::Result<OplogPage>;

    /// Reads the full form of one entry that a page omitted.
    async fn read_entry(
        &self,
        oplog_index: u64,
        component_revision: ComponentRevision,
    ) -> anyhow::Result<PublicOplogEntryWithIndex>;
}

#[derive(Debug, Clone)]
pub struct OplogExportOptions {
    pub keep: KeepFlags,
    /// The largest omitted entry, in bytes, that the export reads in full.
    pub max_entry_bytes: u64,
    pub from: Option<u64>,
    pub query: Option<String>,
    pub page_size: u64,
    pub cli_version: String,
    pub generated_at: String,
}

/// Receives the documents of an export in order: one header, the entries in index order and
/// one coverage document. A failed export never calls `coverage`.
pub trait OplogExportSink {
    fn header(&mut self, header: &OplogExportHeader) -> anyhow::Result<()>;
    fn entry(&mut self, entry: &ReportEntry) -> anyhow::Result<()>;
    fn coverage(&mut self, coverage: &OplogExportCoverage) -> anyhow::Result<()>;
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OplogExportError {
    #[error("The agent was deleted and created again during the export")]
    RootChanged,
    #[error(
        "The Create entry of the agent has {size} bytes, more than --max-entry-bytes ({max_entry_bytes})"
    )]
    CreateTooLarge { size: u64, max_entry_bytes: u64 },
    #[error("The oplog of the agent has no Create entry at index 1")]
    CreateMissing,
    /// A failed read. It holds only the HTTP status, because the error text of a service can
    /// quote the agent ID, the component and environment IDs or other upstream text.
    #[error("A read of the agent failed with HTTP status {status}")]
    ReadFailed { status: u16 },
    #[error("A read of the agent failed before the server answered")]
    ReadFailedWithoutResponse,
}

/// The rendered entries of one page, the cursor of the next page and the coverage of the page.
#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub entries: Vec<ReportEntry>,
    pub next: Option<OplogCursor>,
    pub coverage: OplogExportCoverage,
}

/// The state of one export, or of one served viewer: the reads, the obfuscation context, the
/// pinned root and the function names of the `Start` entries seen so far.
pub struct OplogExportSession<R: OplogReads> {
    reads: R,
    ctx: ObfuscationContext<'static>,
    options: OplogExportOptions,
    fingerprint: Uuid,
    root: ReportAgentRef,
    /// The function name of each `Start` index that the session read; `None` when the `Start`
    /// is unknown (not read, failed to read, or not a `Start`).
    start_functions: HashMap<u64, Option<String>>,
}

impl<R: OplogReads> OplogExportSession<R> {
    /// Reads the `Create` entry of the agent and pins the root to its `instance_id`.
    pub async fn pin(
        reads: R,
        key: &'static TokenKey,
        options: OplogExportOptions,
    ) -> anyhow::Result<Self> {
        let ctx = ObfuscationContext::new(key, options.keep);
        let (fingerprint, agent_id) = read_create(&reads, options.max_entry_bytes).await?;
        let root = agent_reference(&ctx, &agent_id);
        Ok(Self {
            reads,
            ctx,
            options,
            fingerprint,
            root,
            start_functions: HashMap::new(),
        })
    }

    pub fn header(&self) -> OplogExportHeader {
        OplogExportHeader {
            obfuscate: true,
            keep_scalars: self.options.keep.scalars,
            keep_shape: self.options.keep.shape,
            keep_error_text: self.options.keep.error_text,
            filtered: self.options.query.is_some(),
            root: self.root.clone(),
            cli_version: self.options.cli_version.clone(),
            generated_at: self.options.generated_at.clone(),
        }
    }

    pub fn empty_coverage(&self) -> OplogExportCoverage {
        let keep = self.options.keep;
        let notes = if keep.scalars && !keep.shape {
            vec![KEEP_SCALARS_WITHOUT_SHAPE_NOTE.to_string()]
        } else {
            vec![]
        };
        OplogExportCoverage::new(notes)
    }

    /// Fails with [`OplogExportError::RootChanged`] when the `Create` entry has another
    /// `instance_id` than at the pin. A failed read fails too.
    pub async fn check_root(&self) -> anyhow::Result<()> {
        let (fingerprint, _) = read_create(&self.reads, self.options.max_entry_bytes).await?;
        self.check_fingerprint(fingerprint)?;
        Ok(())
    }

    /// Fails with [`OplogExportError::RootChanged`] when `fingerprint`, such as the one of the
    /// agent metadata, is not the `instance_id` of the pinned root.
    pub fn check_fingerprint(&self, fingerprint: Uuid) -> Result<(), OplogExportError> {
        if fingerprint != self.fingerprint {
            return Err(OplogExportError::RootChanged);
        }
        Ok(())
    }

    /// Reads and renders one page.
    pub async fn render_page(
        &mut self,
        from: Option<u64>,
        cursor: Option<&OplogCursor>,
        count: u64,
        query: Option<&str>,
    ) -> anyhow::Result<RenderedPage> {
        let page = self.reads.read_page(from, count, cursor, query).await?;
        let mut coverage = self.empty_coverage();
        let mut entries = Vec::with_capacity(page.entries.len());
        for page_entry in page.entries {
            let entry = match page_entry {
                PublicOplogPageEntry::Included(entry) => {
                    self.render_full(&entry, &mut coverage).await
                }
                PublicOplogPageEntry::Omitted(omitted) => {
                    self.render_omitted_entry(omitted, &mut coverage).await
                }
            };
            entries.push(entry);
        }
        coverage.update_incomplete();
        Ok(RenderedPage {
            entries,
            next: page.next,
            coverage,
        })
    }

    async fn render_omitted_entry(
        &mut self,
        omitted: OmittedOplogEntry,
        coverage: &mut OplogExportCoverage,
    ) -> ReportEntry {
        let index = omitted.oplog_index.as_u64();
        let reason = if omitted.size <= self.options.max_entry_bytes {
            match self
                .reads
                .read_entry(index, omitted.component_revision)
                .await
            {
                Ok(entry) => return self.render_full(&entry, coverage).await,
                Err(error) => {
                    tracing::debug!(
                        oplog_index = index,
                        error = format!("{error:#}"),
                        "Failed to read an omitted oplog entry for the export"
                    );
                    OmittedReason::HydrationFailed
                }
            }
        } else {
            OmittedReason::TooLarge
        };
        if omitted.entry_type == "Start" {
            self.start_functions.insert(index, None);
        }
        coverage.hydration_failures.push(OmittedEntryRef {
            index,
            kind: omitted.entry_type.clone(),
            size: omitted.size,
            reason,
        });
        render_omitted(&self.ctx, &omitted, reason)
    }

    async fn render_full(
        &mut self,
        entry: &PublicOplogEntryWithIndex,
        coverage: &mut OplogExportCoverage,
    ) -> ReportEntry {
        let index = entry.oplog_index.as_u64();
        let start_index = match &entry.entry {
            PublicOplogEntry::Start(params) => {
                self.start_functions
                    .insert(index, Some(params.function_name.clone()));
                None
            }
            PublicOplogEntry::End(params) => Some(params.start_index.as_u64()),
            PublicOplogEntry::Cancelled(params) => Some(params.start_index.as_u64()),
            _ => None,
        };
        let start_function = match start_index {
            Some(start_index) => self.start_function(start_index).await,
            None => None,
        };
        let rendered = render_entry(&self.ctx, entry, start_function.as_deref());
        if rendered.provenance_unknown
            && let Some(start_index) = start_index
        {
            coverage
                .start_unknown
                .push(StartUnknownRef { index, start_index });
        }
        for value in rendered.schema_less_rpc_values {
            coverage.schema_less_rpc_values.push(value);
        }
        rendered.entry
    }

    /// The function name of the `Start` at `start_index`. The session reads each `Start` at most
    /// once, with no query, and reads an omitted `Start` only within `--max-entry-bytes`.
    async fn start_function(&mut self, start_index: u64) -> Option<String> {
        if let Some(known) = self.start_functions.get(&start_index) {
            return known.clone();
        }
        let function = self.read_start_function(start_index).await;
        self.start_functions.insert(start_index, function.clone());
        function
    }

    async fn read_start_function(&self, start_index: u64) -> Option<String> {
        let page = match self.reads.read_page(Some(start_index), 1, None, None).await {
            Ok(page) => page,
            Err(error) => {
                tracing::debug!(
                    start_index,
                    error = format!("{error:#}"),
                    "Failed to read the Start entry of an End for the export"
                );
                return None;
            }
        };
        let entry = match page.entries.into_iter().next()? {
            PublicOplogPageEntry::Included(entry) => entry,
            PublicOplogPageEntry::Omitted(omitted)
                if omitted.oplog_index.as_u64() == start_index
                    && omitted.entry_type == "Start"
                    && omitted.size <= self.options.max_entry_bytes =>
            {
                self.reads
                    .read_entry(start_index, omitted.component_revision)
                    .await
                    .ok()?
            }
            PublicOplogPageEntry::Omitted(_) => return None,
        };
        match entry.entry {
            PublicOplogEntry::Start(params) if entry.oplog_index.as_u64() == start_index => {
                Some(params.function_name)
            }
            _ => None,
        }
    }
}

/// Reads the `Create` entry at index 1 and returns its `instance_id` and agent ID.
async fn read_create<R: OplogReads>(
    reads: &R,
    max_entry_bytes: u64,
) -> anyhow::Result<(Uuid, golem_common::model::AgentId)> {
    let page = reads.read_page(Some(1), 1, None, None).await?;
    let entry = match page.entries.into_iter().next() {
        Some(PublicOplogPageEntry::Included(entry)) => entry,
        Some(PublicOplogPageEntry::Omitted(omitted))
            if omitted.oplog_index.as_u64() == 1 && omitted.entry_type == "Create" =>
        {
            if omitted.size > max_entry_bytes {
                return Err(OplogExportError::CreateTooLarge {
                    size: omitted.size,
                    max_entry_bytes,
                }
                .into());
            }
            reads.read_entry(1, omitted.component_revision).await?
        }
        _ => return Err(OplogExportError::CreateMissing.into()),
    };
    match entry.entry {
        PublicOplogEntry::Create(create) if entry.oplog_index.as_u64() == 1 => {
            Ok((create.instance_id, create.agent_id))
        }
        _ => Err(OplogExportError::CreateMissing.into()),
    }
}

/// Exports the oplog of one agent into the sink: the header, every entry from `options.from`
/// or of `options.query` in index order, then, after a final root check, the coverage.
pub async fn export_oplog<R: OplogReads>(
    reads: R,
    key: &'static TokenKey,
    options: OplogExportOptions,
    sink: &mut impl OplogExportSink,
) -> anyhow::Result<()> {
    let from = options.from;
    let query = options.query.clone();
    let page_size = options.page_size;
    let mut session = OplogExportSession::pin(reads, key, options).await?;
    sink.header(&session.header())?;

    let mut coverage = session.empty_coverage();
    let mut cursor = None;
    loop {
        let page = session
            .render_page(from, cursor.as_ref(), page_size, query.as_deref())
            .await?;
        for entry in &page.entries {
            sink.entry(entry)?;
        }
        coverage.merge(page.coverage);
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    session.check_root().await?;
    sink.coverage(&coverage)
}

#[cfg(test)]
mod tests;
