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
//!
//! A bounded report, [`export_oplog_range`], reads one closed range of the oplog within one scan
//! allowance of the server, and builds the whole report in memory: it fails instead of returning
//! a part of the range.

use crate::oplog_export::model::{
    KEEP_SCALARS_WITHOUT_SHAPE_NOTE, OmittedEntryRef, OmittedReason, OplogExportCoverage,
    OplogExportHeader, OplogExportReport, OplogReportHeader, ReportAgentRef, ReportEntry,
    StartUnknownRef,
};
use crate::oplog_export::obfuscator::{
    KeepFlags, ObfuscationContext, OwnAgent, TokenKey, own_agent_reference, render_entry,
    render_omitted,
};
use async_trait::async_trait;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::public_oplog_entry::CreateParams;
use golem_common::model::oplog::{
    OmittedOplogEntry, OplogCursor, OplogScan, OplogScanAllowance, PublicOplogEntry,
    PublicOplogEntryWithIndex, PublicOplogPageEntry,
};
use std::collections::HashMap;
use uuid::Uuid;

/// One page of an oplog read.
#[derive(Debug, Clone)]
pub struct OplogPage {
    pub entries: Vec<PublicOplogPageEntry>,
    /// The cursor of the next page; `None` after the last page.
    pub next: Option<OplogCursor>,
    /// The last index of the oplog when the server read the page.
    pub last_index: u64,
}

/// One page of an oplog range read, with the scan accounting of the server.
#[derive(Debug, Clone)]
pub struct OplogRangePage {
    pub entries: Vec<PublicOplogPageEntry>,
    /// The cursor of the next page of the range; `None` when the read reached the end of the
    /// range, or when the server spent the allowance before it found where the range starts.
    pub next: Option<OplogCursor>,
    pub scan: OplogScan,
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

    /// Reads one page of the range from `from` to `to`, inclusive, within `allowance`, or
    /// within the configured allowance of the server without one. A failed read is not retried,
    /// because the server can have spent allowance on it.
    async fn read_range_page(
        &self,
        from: u64,
        to: u64,
        count: u64,
        cursor: Option<&OplogCursor>,
        allowance: Option<OplogScanAllowance>,
    ) -> anyhow::Result<OplogRangePage>;

    /// Reads the full form of one entry that a page omitted.
    async fn read_entry(
        &self,
        oplog_index: u64,
        component_revision: ComponentRevision,
    ) -> anyhow::Result<PublicOplogEntryWithIndex>;
}

/// The default largest omitted entry, in bytes, that an export reads in full.
pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

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
    /// A read refused its position in the oplog as stale: the oplog no longer has the entries
    /// that an earlier read of the export saw.
    #[error("A read of the agent found the position of the export stale")]
    StaleCursor,
    /// A read reached below the retained start of the oplog. The text is fixed, because the error
    /// text of the service names the agent.
    #[error(
        "The oplog of the agent starts after the requested index. Export again from a later index"
    )]
    Expired,
    #[error("The range must have 1 <= from <= to, but it is from {from} to {to}")]
    RangeInvalid { from: u64, to: u64 },
    #[error("The range ends at {to}, after the last oplog index {last_index} of the agent")]
    RangePastEnd { to: u64, last_index: u64 },
    #[error(
        "The scan budget of the server ran out before the export read the whole range from {from} to {to}. Narrow `from`/`to`, or use `golem agent oplog --obfuscate`"
    )]
    ScanBudgetExceeded { from: u64, to: u64 },
    #[error(
        "The report is larger than the limit of {limit_bytes} bytes. Narrow `from`/`to`, or use `golem agent oplog --obfuscate`"
    )]
    OutputTooLarge {
        limit_bytes: usize,
        /// The number of entries of the range, when the export read the whole range.
        matched_entries: Option<u64>,
    },
    #[error("The server returned an oplog page that does not continue the range")]
    RangePageInvalid,
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
    /// The last index of the oplog at the pin.
    last_index: u64,
    /// The function name of each `Start` index that the session read and whose `End` or
    /// `Cancelled` it has not rendered yet; `None` when the `Start` is unknown (not read, failed
    /// to read, or not a `Start`). The `End` or `Cancelled` removes the entry, so the map holds
    /// only open invocations.
    start_functions: HashMap<u64, Option<String>>,
}

impl<R: OplogReads> OplogExportSession<R> {
    /// Reads the `Create` entry of the agent and pins the root to its `instance_id`. The root
    /// reference holds the agent ID and the agent type name of the `Create` entry.
    pub async fn pin(
        reads: R,
        key: &'static TokenKey,
        options: OplogExportOptions,
    ) -> anyhow::Result<Self> {
        let (create, last_index) = read_create(&reads, options.max_entry_bytes).await?;
        let own_agent = OwnAgent {
            agent_id: create.agent_id,
            agent_type: create.agent_type_name,
        };
        let ctx = ObfuscationContext::new(key, options.keep, own_agent);
        let fingerprint = create.instance_id;
        let root = own_agent_reference(&ctx);
        Ok(Self {
            reads,
            ctx,
            options,
            fingerprint,
            root,
            last_index,
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
        let (create, _) = read_create(&self.reads, self.options.max_entry_bytes).await?;
        self.check_fingerprint(create.instance_id)?;
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
            entries.push(self.render_entry(page_entry, &mut coverage).await);
        }
        coverage.update_incomplete();
        Ok(RenderedPage {
            entries,
            next: page.next,
            coverage,
        })
    }

    /// Renders one entry of a page, and adds what it does not show in full to `coverage`.
    async fn render_entry(
        &mut self,
        page_entry: PublicOplogPageEntry,
        coverage: &mut OplogExportCoverage,
    ) -> ReportEntry {
        match page_entry {
            PublicOplogPageEntry::Included(entry) => self.render_full(&entry, coverage).await,
            PublicOplogPageEntry::Omitted(omitted) => {
                self.render_omitted_entry(omitted, coverage).await
            }
        }
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
        for mismatch in rendered.host_type_mismatches {
            coverage.host_type_mismatches.push(mismatch);
        }
        rendered.entry
    }

    /// The function name of the `Start` at `start_index`, for its `End` or `Cancelled`. The
    /// session takes it from the map, or reads the `Start` with no query, and reads an omitted
    /// `Start` only within `--max-entry-bytes`.
    async fn start_function(&mut self, start_index: u64) -> Option<String> {
        match self.start_functions.remove(&start_index) {
            Some(known) => known,
            None => self.read_start_function(start_index).await,
        }
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

/// Reads the `Create` entry at index 1 and returns its `instance_id`, the agent ID and the last
/// index of the oplog.
async fn read_create<R: OplogReads>(
    reads: &R,
    max_entry_bytes: u64,
) -> anyhow::Result<(Box<CreateParams>, u64)> {
    let page = reads.read_page(Some(1), 1, None, None).await?;
    let last_index = page.last_index;
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
            Ok((Box::new(create), last_index))
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

/// The closed range of oplog indexes that a bounded report selects; both ends are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OplogRange {
    pub from: u64,
    pub to: u64,
}

/// Exports one closed range of the oplog of one agent as one report, built in memory.
///
/// The range must be within the oplog at the pin of the root. The reads of the range share one
/// scan allowance of the server: the first read gets the configured allowance, and each later
/// read passes on what the reads before it left. When the allowance runs out before the end of
/// the range, the export fails with [`OplogExportError::ScanBudgetExceeded`].
///
/// `document_bytes` is the size of the document of a report, with its newline. When the report
/// cannot fit in `limit_bytes`, the export fails with [`OplogExportError::OutputTooLarge`] and
/// does no more reads: after each entry it compares a lower bound of the size with the limit,
/// and after the last read the exact size. The root is checked again after the size.
pub async fn export_oplog_range<R: OplogReads>(
    reads: R,
    key: &'static TokenKey,
    options: OplogExportOptions,
    range: OplogRange,
    limit_bytes: usize,
    document_bytes: impl Fn(&OplogExportReport) -> anyhow::Result<usize>,
) -> anyhow::Result<OplogExportReport> {
    let OplogRange { from, to } = range;
    if from < 1 || from > to {
        return Err(OplogExportError::RangeInvalid { from, to }.into());
    }
    let page_size = options.page_size;
    let mut session = OplogExportSession::pin(reads, key, options).await?;
    if to > session.last_index {
        return Err(OplogExportError::RangePastEnd {
            to,
            last_index: session.last_index,
        }
        .into());
    }
    let mut report = OplogExportReport {
        header: OplogReportHeader {
            export: session.header(),
            from,
            to,
        },
        entries: Vec::new(),
        coverage: session.empty_coverage(),
    };
    // A lower bound of the size of the document: the document without its entries, with the
    // coverage so far, and the entries with their separators.
    let mut frame_bytes = report_frame_bytes(&report, &document_bytes)?;
    let mut frame_coverage = report.coverage.clone();
    let mut entry_bytes = 0usize;

    let mut cursor: Option<OplogCursor> = None;
    let mut allowance: Option<OplogScanAllowance> = None;
    // The first index that the next page may hold.
    let mut position = from;
    loop {
        let page = session
            .reads
            .read_range_page(from, to, page_size, cursor.as_ref(), allowance)
            .await?;
        let granted = match allowance {
            Some(allowance) => OplogScanAllowance {
                entries: allowance.entries.min(page.scan.limit.entries),
                millis: allowance.millis.min(page.scan.limit.millis),
            },
            None => page.scan.limit,
        };
        let remaining = granted.after(&page.scan);

        for entry in &page.entries {
            let index = entry.oplog_index().as_u64();
            if index != position || index > to {
                return Err(OplogExportError::RangePageInvalid.into());
            }
            position = index + 1;
        }
        let complete = page.next.is_none() && !page.scan.exhausted;
        for page_entry in page.entries {
            let index = page_entry.oplog_index().as_u64();
            let entry = session.render_entry(page_entry, &mut report.coverage).await;
            let separator = usize::from(!report.entries.is_empty());
            entry_bytes = entry_bytes.saturating_add(serde_json::to_vec(&entry)?.len() + separator);
            report.entries.push(entry);
            if report.coverage != frame_coverage {
                frame_bytes = report_frame_bytes(&report, &document_bytes)?;
                frame_coverage = report.coverage.clone();
            }
            if frame_bytes.saturating_add(entry_bytes) > limit_bytes {
                return Err(OplogExportError::OutputTooLarge {
                    limit_bytes,
                    matched_entries: (index == to).then_some(report.entries.len() as u64),
                }
                .into());
            }
        }
        match page.next {
            None if complete => {
                if position != to + 1 {
                    return Err(OplogExportError::RangePageInvalid.into());
                }
                break;
            }
            None => return Err(OplogExportError::ScanBudgetExceeded { from, to }.into()),
            Some(next) => {
                if page.scan.exhausted || remaining.is_spent() {
                    return Err(OplogExportError::ScanBudgetExceeded { from, to }.into());
                }
                let started_at = cursor.as_ref().map_or(from, |c| c.next_oplog_index);
                if next.next_oplog_index <= started_at || next.next_oplog_index != position {
                    return Err(OplogExportError::RangePageInvalid.into());
                }
                cursor = Some(next);
                allowance = Some(remaining);
            }
        }
    }

    report.coverage.update_incomplete();
    if document_bytes(&report)? > limit_bytes {
        return Err(OplogExportError::OutputTooLarge {
            limit_bytes,
            matched_entries: Some(report.entries.len() as u64),
        }
        .into());
    }
    session.check_root().await?;
    Ok(report)
}

/// The size of the document of `report` without its entries, with the coverage as the report
/// would state it.
fn report_frame_bytes(
    report: &OplogExportReport,
    document_bytes: impl Fn(&OplogExportReport) -> anyhow::Result<usize>,
) -> anyhow::Result<usize> {
    let mut frame = OplogExportReport {
        header: report.header.clone(),
        entries: Vec::new(),
        coverage: report.coverage.clone(),
    };
    frame.coverage.update_incomplete();
    document_bytes(&frame)
}

#[cfg(test)]
mod tests;
