// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use golem_common::model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, StreamExportForkAdmittedRecord, StreamExportForkCandidate,
    StreamSessionExpiryPolicy,
};
use golem_common::model::{AgentId, ExportForkAdmissions, OplogIndex};

const MAX_TTL_SECONDS: u64 = 100 * 365 * 24 * 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    Reserved,
    Existing { oplog_index: OplogIndex },
    Conflict,
    Expired,
    InvalidExpiry,
    LimitReached,
    RateLimited { retry_after_seconds: u64 },
}

fn credit(state: &ExportForkAdmissions, limit: u32, now: u64) -> u64 {
    let capacity = u64::from(limit) * 1000;
    state.credit_millis.map_or(capacity, |credit| {
        credit
            .saturating_add(
                now.saturating_sub(state.updated_millis)
                    .saturating_mul(u64::from(limit)),
            )
            .min(capacity)
    })
}

/// Read-only preflight. Only a committed admission record charges either budget.
pub fn check(
    state: &ExportForkAdmissions,
    target: &AgentId,
    session: &str,
    session_limit: u32,
    rate_limit: u32,
    now: u64,
) -> Option<Admission> {
    if state.reservations.contains_key(target) {
        return None;
    }
    if state.session_counts.get(session).copied().unwrap_or(0) >= session_limit {
        return Some(Admission::LimitReached);
    }
    (credit(state, rate_limit, now) < 1000).then_some(Admission::RateLimited {
        retry_after_seconds: 1,
    })
}

/// Decides admission against an attached status while the worker instance lock is held.
/// The returned record must be committed and folded before that lock is released.
pub fn reserve(
    state: &ExportForkAdmissions,
    target: AgentId,
    request_hash: Vec<u8>,
    candidate: StreamExportForkCandidate,
    session_limit: u32,
    rate_limit: u32,
    now: u64,
) -> Result<StreamExportForkAdmittedRecord, Admission> {
    if let Some(record) = state.reservations.get(&target) {
        return Err(
            if record.request_hash == request_hash && record.session == candidate.export.session {
                Admission::Existing {
                    oplog_index: record.oplog_index,
                }
            } else {
                Admission::Conflict
            },
        );
    }
    if let Some(rejection) = check(
        state,
        &target,
        &candidate.export.session,
        session_limit,
        rate_limit,
        now,
    ) {
        return Err(rejection);
    }
    match candidate.expiry_policy {
        StreamSessionExpiryPolicy::None => {}
        StreamSessionExpiryPolicy::Sliding { ttl_seconds } => {
            if ttl_seconds > MAX_TTL_SECONDS {
                return Err(Admission::InvalidExpiry);
            }
            let deadline = ttl_seconds
                .checked_mul(1_000)
                .and_then(|ttl| now.checked_add(ttl))
                .ok_or(Admission::InvalidExpiry)?;
            if i64::try_from(deadline)
                .ok()
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .is_none()
            {
                return Err(Admission::InvalidExpiry);
            }
        }
        StreamSessionExpiryPolicy::Absolute { expires_at_millis } => {
            if expires_at_millis <= now {
                return Err(Admission::Expired);
            }
            if i64::try_from(expires_at_millis)
                .ok()
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .is_none()
            {
                return Err(Admission::InvalidExpiry);
            }
        }
    }
    Ok(StreamExportForkAdmittedRecord {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        target,
        request_hash,
        candidate,
        updated_millis: now.max(state.updated_millis),
        credit_millis: credit(state, rate_limit, now) - 1000,
    })
}
