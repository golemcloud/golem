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

use super::*;
use golem_common::model::oplog::{
    PublicAgentEntityKind, PublicEntityCallMode, PublicEntityInvocation,
    PublicEntityInvocationContext, PublicEntityInvocationOperation, PublicOplogEntryAttribution,
    PublicOplogEntryWithIndex,
};

impl TryFrom<PublicOplogEntryWithIndex> for oplog::AttributedPublicOplogEntry {
    type Error = String;

    fn try_from(entry: PublicOplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            attribution: entry.attribution.into(),
            entry: entry.entry.try_into()?,
        })
    }
}

impl From<PublicOplogEntryAttribution> for oplog::PublicOplogEntryAttribution {
    fn from(attribution: PublicOplogEntryAttribution) -> Self {
        match attribution {
            PublicOplogEntryAttribution::Agent(_) => Self::Agent,
            PublicOplogEntryAttribution::Entity(context) => Self::Entity(context.into()),
        }
    }
}

impl From<PublicEntityInvocationContext> for oplog::PublicEntityInvocationContext {
    fn from(context: PublicEntityInvocationContext) -> Self {
        Self {
            invocation: context.invocation.into(),
            ancestors: context.ancestors.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<PublicEntityInvocation> for oplog::PublicEntityInvocation {
    fn from(invocation: PublicEntityInvocation) -> Self {
        Self {
            entity: oplog::PublicAgentEntity {
                kind: match invocation.entity.kind {
                    PublicAgentEntityKind::Tool => oplog::PublicAgentEntityKind::Tool,
                    PublicAgentEntityKind::ToolMiddleware => {
                        oplog::PublicAgentEntityKind::ToolMiddleware
                    }
                },
                name: invocation.entity.name,
            },
            start_index: invocation.start_index.as_u64(),
            call_mode: match invocation.call_mode {
                PublicEntityCallMode::Synchronous => oplog::PublicEntityCallMode::Synchronous,
                PublicEntityCallMode::Asynchronous => oplog::PublicEntityCallMode::Asynchronous,
                PublicEntityCallMode::FireAndForget => oplog::PublicEntityCallMode::FireAndForget,
            },
            operation: invocation.operation.map(|operation| match operation {
                PublicEntityInvocationOperation::Tool(tool) => {
                    oplog::PublicEntityInvocationOperation::Tool(
                        oplog::PublicToolInvocationOperation {
                            command_path: tool.command_path,
                            has_stdin: tool.has_stdin,
                            has_stdout: tool.has_stdout,
                            declares_stdout: tool.declares_stdout,
                            has_stderr: tool.has_stderr,
                            declares_stderr: tool.declares_stderr,
                        },
                    )
                }
            }),
        }
    }
}
