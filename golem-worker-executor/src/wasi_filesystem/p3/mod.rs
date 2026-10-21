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

use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::durable_host::authorization::targets::CanonicalGuestPath;
use crate::durable_host::concurrent::{AccessClaimOptions, Cancellable, DropPolicy};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_p3_view, durable_worker_ctx, observe_function_call,
    observe_function_call_store, reserve_invocation_id_before_await,
};
use crate::durable_host::tail_work::TailActivity;
use crate::durable_host::{
    CallReplayOutcome, DurabilityHost, DurableCallSession, LiveAuthorizationPermit, NotCancellable,
};
use crate::services::agent_filesystem::{
    self as agent_filesystem, AttributeChanges, Attributes as AgentAttributes,
    Error as AgentFilesystemError, File as AgentFile, FilesystemGenerationHandle,
    FilesystemStorageError, NamespaceEdit, NewObject, ObjectKind, OpenNode, PathTarget,
    SymlinkTarget, Target as AgentTarget, TimeChange, TimeChanges, WritePlacement, WriteResult,
};
use crate::wasi_filesystem::{
    AgentDescriptor, AgentOpenRequest, AgentOpenRouteError, FilesystemInterruptSignal,
    advance_write_placement, agent_descriptor_guest_path, calculate_metadata_hash_parts,
    delete_agent_descriptor, filesystem_permission_targets, flush_level, get_agent_descriptor,
    observe_filesystem_operation, permission_check_request, push_agent_descriptor,
    resize_attribute_changes, route_agent_flush, route_agent_namespace_edit, route_agent_open,
    route_agent_set_attributes, route_agent_write, run_agent_filesystem_call,
};
use crate::workerctx::WorkerCtx;
use bytes::{Bytes, BytesMut};
use golem_common::model::card::{FilesystemVerb, PermissionTarget};
use golem_common::model::entity::FilesystemCapability;
use golem_common::model::oplog::host_functions::{
    FilesystemPermissionCheck, P3FilesystemTypesDescriptorAppendViaStream,
    P3FilesystemTypesDescriptorStat, P3FilesystemTypesDescriptorStatAt,
    P3FilesystemTypesDescriptorWriteViaStream,
};
use golem_common::model::oplog::types::{SerializableFileTimes, SerializableP3FileSystemError};
use golem_common::model::oplog::{
    DeniablePair, DurableFunctionType, HostRequestFileSystemPath, HostRequestNoInput,
    HostResponseP3FileSystemStat, HostResponseP3FileSystemWriteAdmission,
    HostResponsePermissionCheck,
};
use golem_service_base::error::worker_executor::InterruptKind;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureReader, Linker, Resource, Source,
    StreamConsumer, StreamProducer, StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::filesystem::{Descriptor, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::{preopens, types};
use wasmtime_wasi::p3::filesystem::{FilesystemError, FilesystemResult};

mod access;
mod read;
mod routes;
mod stat;
mod translate;
mod write;

use access::*;
use read::*;
pub(crate) use routes::*;
use stat::*;
use translate::*;
use write::*;

/// Registers the WASI P3 filesystem types and preopens host interfaces.
/// P3 accessors route descriptor ownership through `DurableP3` and the shared agent-filesystem table.
pub(crate) fn add_to_linker<Ctx: WorkerCtx>(linker: &mut Linker<Ctx>) -> wasmtime::Result<()> {
    types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    preopens::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    Ok(())
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<types::ErrorCode> {
        observe_function_call(&*self.0, "filesystem::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiFilesystemView::filesystem(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptor for DurableP3View<'_, Ctx> {
    fn drop(&mut self, fd: Resource<Descriptor>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "filesystem::types::descriptor", "drop");
        delete_agent_descriptor(self.0.durable_ctx_mut(), fd)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        observe_function_call(&*self.0, "filesystem::preopens", "get-directories");
        let ctx = self.0.durable_ctx_mut();
        if ctx.filesystem_capability() == FilesystemCapability::Incapable {
            return Ok(Vec::new());
        }
        let preopen = ctx.filesystem_preopen();
        Ok(vec![
            (
                crate::wasi_filesystem::push_agent_descriptor(ctx, preopen.clone())?,
                "/".to_string(),
            ),
            (
                crate::wasi_filesystem::push_agent_descriptor(ctx, preopen)?,
                ".".to_string(),
            ),
        ])
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostDescriptorWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        if let Err(error) = authorize_paths(accessor, &[(FilesystemVerb::Read, path)]).await {
            let code = error.downcast()?;
            return accessor.with(|mut store| {
                Ok((
                    StreamReader::new(&mut store, Vec::<u8>::new())?,
                    FutureReader::new(&mut store, async move {
                        Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(code))
                    })?,
                ))
            });
        }
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if descriptor.with_node(|node| !matches!(node, OpenNode::File(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let producer = FilesystemReadProducer::new(
            generation_handle,
            descriptor,
            offset,
            result_tx,
            filesystem_interrupt::<Ctx, U>(accessor),
        )
        .await?;
        let result_interrupt = filesystem_interrupt::<Ctx, U>(accessor);
        accessor.with(|mut store| {
            let mut stream = StreamReader::new(&mut store, producer)?;
            let future = match FutureReader::new(
                &mut store,
                wait_filesystem_task_result(result_rx, result_interrupt),
            ) {
                Ok(future) => future,
                Err(error) => {
                    let _ = stream.close(&mut store);
                    return Err(error);
                }
            };
            Ok((stream, future))
        })
    }

    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: types::Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let authorization_permit =
            match admit_stream_write::<P3FilesystemTypesDescriptorWriteViaStream, Ctx, U>(
                accessor, &fd,
            )
            .await?
            {
                Ok(permit) => permit,
                Err(error) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
            };
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "write-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if !agent_descriptor_flags(&descriptor)
            .map_err(|error| match error.downcast() {
                Ok(error) => wasmtime::Error::msg(format!("{error:?}")),
                Err(error) => error,
            })?
            .contains(types::DescriptorFlags::WRITE)
        {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(
                        types::ErrorCode::NotPermitted,
                    ))
                })
            });
        }
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let result_interrupt = filesystem_interrupt::<Ctx, U>(accessor);
        let future = accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                generation_handle,
                descriptor,
                WritePlacement::At(offset),
                activity,
                authorization_permit,
            ));

            FutureReader::new(
                &mut store,
                wait_filesystem_task_result(result_rx, result_interrupt),
            )
        })?;
        Ok(future)
    }

    async fn append_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let authorization_permit =
            match admit_stream_write::<P3FilesystemTypesDescriptorAppendViaStream, Ctx, U>(
                accessor, &fd,
            )
            .await?
            {
                Ok(permit) => permit,
                Err(error) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
            };
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "append-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if !agent_descriptor_flags(&descriptor)
            .map_err(|error| match error.downcast() {
                Ok(error) => wasmtime::Error::msg(format!("{error:?}")),
                Err(error) => error,
            })?
            .contains(types::DescriptorFlags::WRITE)
        {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(
                        types::ErrorCode::NotPermitted,
                    ))
                })
            });
        }
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let result_interrupt = filesystem_interrupt::<Ctx, U>(accessor);
        let future = accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                generation_handle,
                descriptor,
                WritePlacement::Append,
                activity,
                authorization_permit,
            ));

            FutureReader::new(
                &mut store,
                wait_filesystem_task_result(result_rx, result_interrupt),
            )
        })?;
        Ok(future)
    }

    async fn advise(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        _offset: types::Filesize,
        _length: types::Filesize,
        _advice: types::Advice,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "advise",
            )
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }
        Ok(())
    }

    async fn sync_data(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync-data",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation = descriptor.with_node(|node| route_flush(&generation_handle, node, true))?;
        observe_p3_filesystem(filesystem_interrupt::<Ctx, U>(store), operation).await
    }

    async fn get_flags(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorFlags> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "get-flags",
            )
        });
        let descriptor = accessor
            .with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))
            .map_err(FilesystemError::trap)?;
        agent_descriptor_flags(&descriptor)
    }

    async fn get_type(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorType> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "get-type",
            )
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        Ok(p3_agent_descriptor_type(
            descriptor.with_node(OpenNode::kind),
        ))
    }

    async fn set_size(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, path)]).await?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-size",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }
        let operation =
            descriptor.with_node(|node| route_set_size(&generation_handle, node, size))?;
        observe_p3_filesystem(filesystem_interrupt::<Ctx, U>(accessor), operation).await
    }

    async fn set_times(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times",
            )
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation = descriptor.with_node(|node| {
            route_set_times(
                &generation_handle,
                AgentTarget::Open(node),
                data_access_timestamp,
                data_modification_timestamp,
            )
        })?;
        observe_p3_filesystem(filesystem_interrupt::<Ctx, U>(accessor), operation).await
    }

    async fn read_directory(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let denied = match authorize_paths(accessor, &[(FilesystemVerb::List, path)]).await {
            Ok(_) => None,
            Err(error) => Some(error.downcast()?),
        };
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-directory",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        let call = if let Some(code) = denied {
            Err(code.into())
        } else {
            descriptor.with_node(|node| match node {
                OpenNode::Directory(directory) => {
                    agent_filesystem::list_directory(&generation_handle, directory)
                        .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))
                }
                OpenNode::File(_) => Err(types::ErrorCode::NotDirectory.into()),
            })
        };
        let (mut entries, result) = match call {
            Ok(call) => {
                match observe_filesystem_operation(filesystem_interrupt::<Ctx, U>(accessor), call)
                    .await
                    .map_err(wasmtime::Error::from)?
                {
                    Ok(entries) => match entries
                        .into_iter()
                        .map(|entry| {
                            Ok(types::DirectoryEntry {
                                type_: p3_agent_descriptor_type(entry.kind),
                                name: entry
                                    .name
                                    .into_string()
                                    .map_err(|_| types::ErrorCode::IllegalByteSequence)?,
                            })
                        })
                        .collect::<Result<Vec<_>, types::ErrorCode>>()
                    {
                        Ok(entries) => (entries, Ok(())),
                        Err(error) => (Vec::new(), Err(error)),
                    },
                    Err(error) => (Vec::new(), Err(p3_agent_error(error).downcast()?)),
                }
            }
            Err(error) => (Vec::new(), Err(error.downcast()?)),
        };
        entries.sort_by_key(|entry| entry.name.clone());

        accessor.with(|mut store| {
            let stream = StreamReader::new(&mut store, entries)?;
            let future = FutureReader::new(&mut store, async move {
                Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
            })?;
            Ok::<_, wasmtime::Error>((stream, future))
        })
    }

    async fn sync(store: &Accessor<U, Self>, fd: Resource<Descriptor>) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation =
            descriptor.with_node(|node| route_flush(&generation_handle, node, false))?;
        observe_p3_filesystem(filesystem_interrupt::<Ctx, U>(store), operation).await
    }

    async fn create_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "create-directory-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(store),
            route_create_directory(&generation_handle, target),
        )
        .await
    }

    async fn stat(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorStat> {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)
            .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let permissions = [(FilesystemVerb::Stat, guest_path.clone())];
        let immutable_initial_file =
            p3_is_immutable_initial_file::<Ctx, U>(store, &fd, &guest_path, None).await?;
        let path =
            descriptor_path_from_accessor::<Ctx, U>(store, &fd).map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();

        if immutable_initial_file {
            let _authorization_permit = authorize_paths(store, &permissions).await?;
            let stat = p3_stable_initial_file_stat(run_local_stat::<Ctx, U>(
                store,
                Resource::new_borrow(fd_rep),
            ))
            .await?;
            store.with(|mut access| {
                observe_function_call_store::<Ctx, U>(
                    access.data_mut(),
                    "filesystem::types::descriptor",
                    "stat-async",
                )
            });
            return Ok(stat);
        }

        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);

        let (response, _authorization_permit) =
            run_permission_checked_access::<Ctx, U, P3FilesystemTypesDescriptorStat>(
                store,
                HostRequestFileSystemPath {
                    path: path.to_string_lossy().to_string(),
                },
                &permissions,
                async || {
                    let stat =
                        run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep)).await?;
                    *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                    Ok(HostResponseP3FileSystemStat {
                        result: serialize_stat_result(&stat),
                    })
                },
            )
            .await
            .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match (live_stat, &response.result) {
            (Some(stat), _) => stat,
            (None, Err(error)) => Err(error.clone()),
            (None, Ok(_)) => run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep))
                .await
                .map_err(FilesystemError::trap)?,
        };

        apply_stat_response(stat, response).await
    }

    async fn stat_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::DescriptorStat> {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)
            .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let permissions = [(FilesystemVerb::Stat, guest_path.clone())];
        let immutable_initial_file = p3_is_immutable_initial_file::<Ctx, U>(
            store,
            &fd,
            &guest_path,
            Some((path_flags, path.clone())),
        )
        .await?;
        let full_path = descriptor_path_at_from_accessor::<Ctx, U>(store, &fd, &path)
            .map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();

        if immutable_initial_file {
            let _authorization_permit = authorize_paths(store, &permissions).await?;
            let stat = p3_stable_initial_file_stat(run_local_stat_at::<Ctx, U>(
                store,
                Resource::new_borrow(fd_rep),
                path_flags,
                path.clone(),
            ))
            .await?;
            store.with(|mut access| {
                observe_function_call_store::<Ctx, U>(
                    access.data_mut(),
                    "filesystem::types::descriptor",
                    "stat-at",
                )
            });
            return Ok(stat);
        }

        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);
        let live_path = path.clone();
        let (response, _authorization_permit) =
            run_permission_checked_access::<Ctx, U, P3FilesystemTypesDescriptorStatAt>(
                store,
                HostRequestFileSystemPath {
                    path: full_path.to_string_lossy().to_string(),
                },
                &permissions,
                async || {
                    let stat = run_local_stat_at::<Ctx, U>(
                        store,
                        Resource::new_borrow(fd_rep),
                        path_flags,
                        live_path,
                    )
                    .await?;
                    *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                    Ok(HostResponseP3FileSystemStat {
                        result: serialize_stat_result(&stat),
                    })
                },
            )
            .await
            .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match (live_stat, &response.result) {
            (Some(stat), _) => stat,
            (None, Err(error)) => Err(error.clone()),
            (None, Ok(_)) => {
                run_local_stat_at::<Ctx, U>(store, Resource::new_borrow(fd_rep), path_flags, path)
                    .await
                    .map_err(FilesystemError::trap)?
            }
        };

        apply_stat_response(stat, response).await
    }

    async fn set_times_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, guest_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times-at",
            )
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        let follow = if path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
            agent_filesystem::Follow::Yes
        } else {
            agent_filesystem::Follow::No
        };
        let operation = route_set_times(
            &generation_handle,
            AgentTarget::Path(&target, follow),
            data_access_timestamp,
            data_modification_timestamp,
        )?;
        observe_p3_filesystem(filesystem_interrupt::<Ctx, U>(accessor), operation).await
    }

    async fn link_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let source_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &old_path)?;
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(store, &new_fd, &new_path)?;
        let _authorization_permit = authorize_paths(
            store,
            &[
                (FilesystemVerb::Read, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "link-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let source_descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        let source = agent_path_target(&source_descriptor, old_path)?;
        let destination = agent_path_target(&destination_descriptor, new_path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(store),
            route_hard_link(&generation_handle, source, old_path_flags, destination),
        )
        .await
    }

    async fn open_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let mut permissions = Vec::new();
        if flags.contains(types::DescriptorFlags::READ) {
            permissions.push((FilesystemVerb::Read, guest_path.clone()));
        }
        if flags.contains(types::DescriptorFlags::WRITE)
            || open_flags.intersects(types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE)
        {
            permissions.push((FilesystemVerb::Write, guest_path.clone()));
        }
        if open_flags.contains(types::OpenFlags::DIRECTORY) {
            permissions.push((FilesystemVerb::List, guest_path));
        }
        let _authorization_permit = authorize_paths(accessor, &permissions).await?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "open-at",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let descriptor_path = descriptor.path().join(&path);
        let target = agent_path_target(&descriptor, path)?;
        let opened = route_open(
            &generation_handle,
            target,
            path_flags,
            open_flags,
            flags,
            filesystem_interrupt::<Ctx, U>(accessor),
        )
        .await?;
        push_agent_descriptor_from_accessor(
            accessor,
            AgentDescriptor::new(opened.node, descriptor_path),
        )
    }

    async fn readlink_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Stat, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "readlink-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(store),
            route_symlink_target(&generation_handle, target),
        )
        .await
    }

    async fn remove_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Delete, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "remove-directory-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(store),
            route_remove_directory(&generation_handle, target),
        )
        .await
    }

    async fn rename_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let source_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &old_path)?;
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &new_fd, &new_path)?;
        let _authorization_permit = authorize_paths(
            accessor,
            &[
                (FilesystemVerb::Delete, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let source_descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "rename-at",
            )
        });
        let source = agent_path_target(&source_descriptor, old_path)?;
        let destination = agent_path_target(&destination_descriptor, new_path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(accessor),
            route_rename(&generation_handle, source, destination),
        )
        .await
    }

    async fn symlink_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &new_path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, destination_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "symlink-at",
            )
        });
        let destination = agent_path_target(&descriptor, new_path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(accessor),
            route_create_symlink(&generation_handle, destination, old_path),
        )
        .await
    }

    async fn unlink_file_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Delete, guest_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "unlink-file-at",
            )
        });
        let target = agent_path_target(&descriptor, path)?;
        observe_p3_filesystem(
            filesystem_interrupt::<Ctx, U>(accessor),
            route_unlink(&generation_handle, target, ObjectKind::File),
        )
        .await
    }

    async fn is_same_object(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "is-same-object",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let left =
            store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))?;
        let right =
            store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &other))?;
        let call = AgentDescriptor::with_nodes(&left, &right, |left, right| {
            agent_filesystem::is_same_object(&generation_handle, left, right)
        })?;
        observe_filesystem_operation(filesystem_interrupt::<Ctx, U>(store), call)
            .await
            .map_err(wasmtime::Error::from)?
            .map_err(|error| wasmtime::Error::msg(error.to_string()))
    }

    async fn metadata_hash(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::MetadataHashValue> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "metadata-hash",
            )
        });
        // Computed from the durable stat result so the hash only depends on
        // replay-stable inputs (durable file times + size), matching WASI P2.
        let stat = Self::stat(store, fd).await?;
        metadata_hash_from_stat(&stat)
    }

    async fn metadata_hash_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::MetadataHashValue> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "metadata-hash-at",
            )
        });
        // Computed from the durable stat result so the hash only depends on
        // replay-stable inputs (durable file times + size), matching WASI P2.
        let stat = Self::stat_at(store, fd, path_flags, path).await?;
        metadata_hash_from_stat(&stat)
    }
}

#[cfg(test)]
mod tests;
