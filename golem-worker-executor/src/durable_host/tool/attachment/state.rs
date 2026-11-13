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

use super::{
    AttachmentState, ByteStreamCloseCause, ByteStreamFailure, StreamWriteError, close_cause_eq,
};
use crate::durable_host::tool::durability;
use golem_common::model::OplogIndex;
use golem_common::model::oplog::attachment::ToolAttachmentTerminal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AttachmentMode {
    Pending,
    Live,
    CompletionStaged,
    CompletionPublished,
    TerminalOnly,
    Discard,
}

impl AttachmentMode {
    pub(super) fn chunks_visible(self) -> bool {
        matches!(self, Self::Live | Self::CompletionPublished)
    }

    pub(super) fn terminal_visible(self) -> bool {
        self.chunks_visible() || self == Self::TerminalOnly
    }
}

pub(super) enum AttachmentTerminalState {
    Open,
    OwnerReserved(ToolAttachmentTerminal),
    Appending {
        terminal: ToolAttachmentTerminal,
        failure_item_delivered: bool,
    },
    RecordedPending {
        terminal: ToolAttachmentTerminal,
        end: OplogIndex,
        failure_item_delivered: bool,
    },
    Available {
        terminal: ToolAttachmentTerminal,
        record: TerminalRecord,
        failure_item_delivered: bool,
    },
}

pub(super) enum TerminalRecord {
    Local,
    Recorded(OplogIndex),
}

fn selection(cause: ByteStreamCloseCause, host_resource_exhausted: bool) -> ToolAttachmentTerminal {
    if host_resource_exhausted {
        ToolAttachmentTerminal::HostResourceExhausted
    } else {
        ToolAttachmentTerminal::Application(durability::cause_to_model(&cause))
    }
}

impl AttachmentTerminalState {
    fn selected(&self) -> Option<&ToolAttachmentTerminal> {
        match self {
            Self::Open => None,
            Self::OwnerReserved(terminal)
            | Self::Appending { terminal, .. }
            | Self::RecordedPending { terminal, .. }
            | Self::Available { terminal, .. } => Some(terminal),
        }
    }

    pub(super) fn selected_cause(&self) -> Option<ByteStreamCloseCause> {
        self.selected().map(|terminal| match terminal {
            ToolAttachmentTerminal::Application(cause) => {
                durability::cause_from_model(cause.clone())
            }
            ToolAttachmentTerminal::HostResourceExhausted => {
                ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted)
            }
        })
    }

    pub(super) fn is_selected(&self) -> bool {
        !matches!(self, Self::Open)
    }

    pub(super) fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub(super) fn available_cause(&self) -> Option<ByteStreamCloseCause> {
        self.is_available().then(|| self.selected_cause()).flatten()
    }

    pub(super) fn recorded_end(&self) -> Option<OplogIndex> {
        match self {
            Self::RecordedPending { end, .. }
            | Self::Available {
                record: TerminalRecord::Recorded(end),
                ..
            } => Some(*end),
            _ => None,
        }
    }

    pub(super) fn published_end(&self) -> Option<OplogIndex> {
        self.is_available().then(|| self.recorded_end()).flatten()
    }

    pub(super) fn host_resource_exhausted(&self) -> bool {
        matches!(
            self.selected(),
            Some(ToolAttachmentTerminal::HostResourceExhausted)
        )
    }

    pub(super) fn failure_item_delivered(&self) -> bool {
        match self {
            Self::Appending {
                failure_item_delivered,
                ..
            }
            | Self::RecordedPending {
                failure_item_delivered,
                ..
            }
            | Self::Available {
                failure_item_delivered,
                ..
            } => *failure_item_delivered,
            Self::Open | Self::OwnerReserved(_) => false,
        }
    }

    pub(super) fn mark_failure_delivered(&mut self) {
        match self {
            Self::Appending {
                failure_item_delivered,
                ..
            }
            | Self::RecordedPending {
                failure_item_delivered,
                ..
            }
            | Self::Available {
                failure_item_delivered,
                ..
            } => *failure_item_delivered = true,
            Self::Open | Self::OwnerReserved(_) => {}
        }
    }

    fn check_selection(&self, cause: &ByteStreamCloseCause) -> Result<bool, StreamWriteError> {
        match self.selected_cause() {
            None => Ok(true),
            Some(selected) if close_cause_eq(&selected, cause) => Ok(false),
            Some(selected) => Err(StreamWriteError::Closed(selected)),
        }
    }

    pub(super) fn select_local(
        &mut self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
    ) -> Result<bool, StreamWriteError> {
        if !self.check_selection(&cause)? {
            return Ok(false);
        }
        *self = Self::Available {
            terminal: selection(cause, host_resource_exhausted),
            record: TerminalRecord::Local,
            failure_item_delivered: false,
        };
        Ok(true)
    }

    pub(super) fn accept(
        &mut self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
    ) -> Result<bool, StreamWriteError> {
        if !self.check_selection(&cause)? {
            return Ok(false);
        }
        *self = Self::Appending {
            terminal: selection(cause, host_resource_exhausted),
            failure_item_delivered: false,
        };
        Ok(true)
    }

    pub(super) fn reserve_owner(&mut self, failure: ByteStreamFailure) {
        if matches!(self, Self::Open) {
            let host = matches!(failure, ByteStreamFailure::ResourceExhausted);
            *self = Self::OwnerReserved(selection(ByteStreamCloseCause::Failed(failure), host));
        }
    }

    pub(super) fn accept_owner(
        &mut self,
        failure: ByteStreamFailure,
    ) -> Option<ToolAttachmentTerminal> {
        let terminal = match self {
            Self::Open => {
                let host = matches!(failure, ByteStreamFailure::ResourceExhausted);
                selection(ByteStreamCloseCause::Failed(failure), host)
            }
            Self::OwnerReserved(terminal)
            | Self::Available {
                terminal,
                record: TerminalRecord::Local,
                ..
            } => terminal.clone(),
            _ => return None,
        };
        *self = Self::Appending {
            terminal: terminal.clone(),
            failure_item_delivered: self.failure_item_delivered(),
        };
        Some(terminal)
    }

    pub(super) fn disposal_owned_elsewhere(&self) -> bool {
        matches!(self, Self::OwnerReserved(_))
            || (self.is_available()
                && self.recorded_end().is_none()
                && matches!(
                    self.selected_cause(),
                    Some(ByteStreamCloseCause::ConsumerCancelled)
                ))
    }

    pub(super) fn restore(
        &mut self,
        cause: ByteStreamCloseCause,
        host_resource_exhausted: bool,
        end: OplogIndex,
        pending: bool,
        observed: Option<OplogIndex>,
    ) -> anyhow::Result<()> {
        let terminal = selection(cause, host_resource_exhausted);
        if let Some(selected_end) = self.recorded_end() {
            anyhow::ensure!(
                selected_end == end && self.selected() == Some(&terminal),
                "recorded terminal conflicts with an existing durable selection"
            );
        }
        let failure_item_delivered = observed == Some(end);
        *self = if pending {
            Self::RecordedPending {
                terminal,
                end,
                failure_item_delivered,
            }
        } else {
            Self::Available {
                terminal,
                record: TerminalRecord::Recorded(end),
                failure_item_delivered,
            }
        };
        Ok(())
    }

    pub(super) fn observe(&mut self, end: OplogIndex) {
        if self.recorded_end() == Some(end) {
            self.mark_failure_delivered();
        }
    }

    pub(super) fn publish(&mut self, end: OplogIndex) {
        // Restoration may have replaced the selection since the capability was issued.
        if let Some(terminal) = self.selected().cloned() {
            *self = Self::Available {
                terminal,
                record: TerminalRecord::Recorded(end),
                failure_item_delivered: self.failure_item_delivered(),
            };
        }
    }

    pub(super) fn fence(&mut self) -> bool {
        if self.is_available() {
            return false;
        }
        *self = Self::Available {
            terminal: selection(
                ByteStreamCloseCause::Failed(ByteStreamFailure::Cancelled),
                false,
            ),
            record: TerminalRecord::Local,
            failure_item_delivered: self.failure_item_delivered(),
        };
        true
    }
}

impl AttachmentState {
    pub(super) fn configure_mode(&mut self, mode: AttachmentMode) -> bool {
        if self.mode != AttachmentMode::Pending {
            return false;
        }
        if mode == AttachmentMode::Live {
            self.accepted_completion_bytes = 0;
        }
        self.mode = mode;
        true
    }

    pub(super) fn publish_completion(&mut self) -> bool {
        if self.mode != AttachmentMode::CompletionStaged || !self.terminal.is_selected() {
            return false;
        }
        self.mode = AttachmentMode::CompletionPublished;
        true
    }

    pub(super) fn prepare_terminal_only(&mut self) {
        if self.mode == AttachmentMode::Pending {
            self.chunks.clear();
            self.buffered_bytes = 0;
            self.mode = AttachmentMode::TerminalOnly;
        }
    }

    pub(super) fn publish_no_body(&mut self) -> bool {
        if self.owner_fenced || !self.terminal.is_selected() {
            return false;
        }
        self.prepare_terminal_only();
        self.publish_completion();
        true
    }
}
