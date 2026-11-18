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

//! The obfuscator: renders a public oplog entry as a report entry.
//!
//! Every customer value is normalised to one canonical node and becomes a keyed token, unless
//! a keep flag or a Golem metadata rule shows more. The key is random per CLI process, so
//! tokens are equal inside one export (and one `--serve` session) and cannot be compared across
//! processes.

mod canonical;
mod describe;
mod entry;
mod errors;
mod host;
mod value;

#[cfg(test)]
mod tests;

use crate::oplog_export::model::{ReportAgentRef, ReportToken};
use golem_common::model::AgentId;
use hmac::{Hmac, Mac};
use rand::TryRngCore;
use rand::rngs::OsRng;
use sha2::Sha256;
use std::fmt::{Debug, Formatter};
use std::sync::OnceLock;

pub use describe::describe_schema;
pub use entry::{RenderedEntry, render_entry, render_omitted};
pub use errors::{
    ErrorText, FAILED_UPDATE_DETAIL_CODES, FIXED_ERROR_TEXTS, agent_error_ordinal,
    agent_error_samples, agent_error_text,
};

const TOKEN_CONTEXT: &[u8] = b"golem-oplog-export/v1";

/// The HMAC key of the tokens. Release code gets it only through [`TokenKey::process`]; the
/// key is never stored, logged, serialised or written.
pub struct TokenKey([u8; 32]);

static PROCESS_KEY: OnceLock<TokenKey> = OnceLock::new();

#[cfg(test)]
static PROCESS_KEY_DRAWS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl TokenKey {
    /// The key of this process: drawn from the operating system's random source at the first
    /// call, then the same key for the life of the process.
    pub fn process() -> &'static TokenKey {
        PROCESS_KEY.get_or_init(|| {
            #[cfg(test)]
            PROCESS_KEY_DRAWS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut bytes = [0u8; 32];
            OsRng
                .try_fill_bytes(&mut bytes)
                .expect("Failed to read the operating system's random source");
            TokenKey(bytes)
        })
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        TokenKey(bytes)
    }

    #[cfg(test)]
    pub(crate) fn process_key_draws() -> usize {
        PROCESS_KEY_DRAWS.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Debug for TokenKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenKey(..)")
    }
}

/// The opt-in residual leaks of one export. Every flag is off by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeepFlags {
    /// Numbers, booleans, datetimes, durations and quantities in expanded nodes.
    pub scalars: bool,
    /// Cases, tags, option presence, result branches, lengths and the elements of collections.
    pub shape: bool,
    /// The text of free error messages.
    pub error_text: bool,
}

/// The key and the keep flags of one export.
#[derive(Debug, Clone, Copy)]
pub struct ObfuscationContext<'a> {
    key: &'a TokenKey,
    keep: KeepFlags,
}

impl<'a> ObfuscationContext<'a> {
    pub fn new(key: &'a TokenKey, keep: KeepFlags) -> Self {
        Self { key, keep }
    }

    pub fn keep(&self) -> KeepFlags {
        self.keep
    }

    /// `HMAC-SHA256(key, context ‖ domain ‖ canonical bytes)`, with the domain and the bytes
    /// each written with a `u32` big-endian length prefix, cut to 16 bytes of lower-case hex.
    pub(crate) fn token(&self, domain: &str, canonical_bytes: &[u8]) -> ReportToken {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key.0).expect("HMAC accepts a key of any size");
        mac.update(TOKEN_CONTEXT);
        mac.update(&(domain.len() as u32).to_be_bytes());
        mac.update(domain.as_bytes());
        mac.update(&(canonical_bytes.len() as u32).to_be_bytes());
        mac.update(canonical_bytes);
        let digest = mac.finalize().into_bytes();
        let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        ReportToken(format!("{domain}:{hex}"))
    }
}

/// The reference to an agent: one `agent` token over the `Display` text of its ID. The ID is
/// never parsed, so the reference shows no agent type, constructor parameter or component ID.
pub fn agent_reference(ctx: &ObfuscationContext, agent_id: &AgentId) -> ReportAgentRef {
    let text = agent_id.to_string();
    let mut bytes = Vec::with_capacity(text.len() + 4);
    bytes.extend_from_slice(&(text.len() as u32).to_be_bytes());
    bytes.extend_from_slice(text.as_bytes());
    ReportAgentRef {
        token: ctx.token("agent", &bytes),
    }
}
