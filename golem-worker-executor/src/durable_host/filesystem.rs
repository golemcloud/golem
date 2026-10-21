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

#[async_trait]
impl<Ctx: WorkerCtx> FileSystemReading for DurableWorkerCtx<Ctx> {
    async fn get_file_system_node(
        &self,
        path: &CanonicalFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        use crate::services::agent_filesystem as agent_fs;

        let generation_handle = self.filesystem_generation_handle();
        let relative = PathBuf::from(path.to_rel_string());
        let target = agent_fs::PathTarget::at_root(&generation_handle, relative.clone())
            .map_err(|error| filesystem_read_error(path, error))?;
        let attributes = match agent_fs::attributes(
            &generation_handle,
            agent_fs::Target::Path(&target, agent_fs::Follow::Yes),
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        {
            Ok(attributes) => attributes,
            Err(error) if filesystem_error_is_not_found(&error) => {
                return Ok(GetFileSystemNodeResult::NotFound);
            }
            Err(error) => return Err(filesystem_read_error(path, error)),
        };

        if attributes.kind == agent_fs::ObjectKind::File {
            return Ok(GetFileSystemNodeResult::File(component_file_node(
                relative, attributes,
            )?));
        }
        if attributes.kind != agent_fs::ObjectKind::Directory {
            return Ok(GetFileSystemNodeResult::NotFound);
        }

        let opened = agent_fs::open(
            &generation_handle,
            target,
            agent_fs::OpenOptions::Existing {
                expected: agent_fs::ObjectKind::Directory,
                access: agent_fs::AccessMode::Read,
                follow: agent_fs::Follow::Yes,
            },
        )
        .map_err(|error| filesystem_read_error(path, error))?
        .await
        .map_err(|error| filesystem_read_error(path, error))?;
        let agent_fs::OpenNode::Directory(directory) = opened.node else {
            unreachable!("directory open returned a non-directory node")
        };
        let entries = agent_fs::list_directory(&generation_handle, &directory)
            .map_err(|error| filesystem_read_error(path, error))?
            .await
            .map_err(|error| filesystem_read_error(path, error))?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in entries {
            let entry_relative = relative.join(&entry.name);
            let entry_target =
                agent_fs::PathTarget::at_root(&generation_handle, entry_relative.clone())
                    .map_err(|error| filesystem_read_error(path, error))?;
            let attributes = agent_fs::attributes(
                &generation_handle,
                agent_fs::Target::Path(&entry_target, agent_fs::Follow::Yes),
            )
            .map_err(|error| filesystem_read_error(path, error))?
            .await
            .map_err(|error| filesystem_read_error(path, error))?;
            result.push(component_file_node(entry_relative, attributes)?);
        }
        Ok(GetFileSystemNodeResult::Ok(result))
    }
}

fn filesystem_error_is_not_found(error: &crate::services::agent_filesystem::Error) -> bool {
    matches!(
        error,
        crate::services::agent_filesystem::Error::Sandbox(source)
            if source.io_error().is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    )
}

fn filesystem_read_error(path: &CanonicalFilePath, error: impl Display) -> WorkerExecutorError {
    WorkerExecutorError::FileSystemError {
        path: path.to_string(),
        reason: error.to_string(),
    }
}

pub(in crate::durable_host) fn component_file_node(
    relative: PathBuf,
    attributes: crate::services::agent_filesystem::Attributes,
) -> Result<ComponentFileSystemNode, WorkerExecutorError> {
    use crate::services::agent_filesystem as agent_fs;

    let name = relative
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let last_modified = attributes.modified.unwrap_or(SystemTime::UNIX_EPOCH);
    let details = match attributes.kind {
        agent_fs::ObjectKind::File => ComponentFileSystemNodeDetails::File {
            size: attributes.size,
            permissions: if attributes.read_only {
                golem_common::model::component::AgentFilePermissions::ReadOnly
            } else {
                golem_common::model::component::AgentFilePermissions::ReadWrite
            },
        },
        agent_fs::ObjectKind::Directory | agent_fs::ObjectKind::Symlink => {
            ComponentFileSystemNodeDetails::Directory
        }
    };
    Ok(ComponentFileSystemNode {
        name,
        last_modified,
        details,
    })
}
