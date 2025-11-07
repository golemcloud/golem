// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use chrono::Timelike;
use desert_rust::BinaryCodec;
use golem_wasm::{FromValue, IntoValue};
use std::fmt::Display;
use std::ops::Add;
use std::str::FromStr;

#[cfg(test)]
mod tests {
    use test_r::test;

    use golem_common::model::oplog::types::{
        SerializableDateTime, SerializableIpAddress, SerializableIpAddresses,
        SerializableStreamError,
    };
    use golem_common::model::oplog::{OplogIndex, WorkerError};
    use golem_common::model::{ComponentId, PromiseId, ShardId, WorkerId};
    use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::strategy::LazyJust;
    use std::ops::Add;
    use std::time::{Duration, SystemTime};
    use uuid::Uuid;
    use wasmtime_wasi::p2::bindings::sockets::network::IpAddress;
    use wasmtime_wasi::p2::bindings::{filesystem, sockets};
    use wasmtime_wasi::p2::{FsError, SocketError};
    use wasmtime_wasi::StreamError;

    fn datetime_strat(
    ) -> impl Strategy<Value = wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
                seconds,
                nanoseconds,
            }
        })
    }

    fn systemtime_strat() -> impl Strategy<Value = SystemTime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            SystemTime::UNIX_EPOCH.add(Duration::new(seconds, nanoseconds))
        })
    }

    fn cap_std_systemtime_strat() -> impl Strategy<Value = cap_std::time::SystemTime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            cap_std::time::SystemTime::from_std(
                SystemTime::UNIX_EPOCH.add(Duration::new(seconds, nanoseconds)),
            )
        })
    }

    fn anyhow_strat() -> impl Strategy<Value = anyhow::Error> {
        ".*".prop_map(anyhow::Error::msg)
    }

    fn fserror_strat() -> impl Strategy<Value = FsError> {
        prop_oneof! {
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Access)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::WouldBlock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Already)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::BadDescriptor)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Busy)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Deadlock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Quota)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Exist)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::FileTooLarge)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IllegalByteSequence)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InProgress)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Interrupted)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Invalid)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Io)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IsDirectory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Loop)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TooManyLinks)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::MessageSize)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NameTooLong)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoDevice)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoEntry)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoLock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientMemory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientSpace)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotDirectory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotEmpty)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotRecoverable)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Unsupported)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoTty)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoSuchDevice)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Overflow)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotPermitted)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Pipe)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::ReadOnly)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InvalidSeek)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TextFileBusy)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::CrossDevice)),
            anyhow_strat().prop_map(FsError::trap),
        }
    }

    fn uuid_strat() -> impl Strategy<Value = Uuid> {
        (any::<u64>(), any::<u64>()).prop_map(|(a, b)| Uuid::from_u64_pair(a, b))
    }

    fn componentid_strat() -> impl Strategy<Value = ComponentId> {
        uuid_strat().prop_map(ComponentId)
    }

    fn workerid_strat() -> impl Strategy<Value = WorkerId> {
        (componentid_strat(), ".+").prop_map(|(component_id, worker_name)| WorkerId {
            component_id,
            worker_name,
        })
    }

    fn promiseid_strat() -> impl Strategy<Value = PromiseId> {
        (workerid_strat(), any::<u64>()).prop_map(|(worker_id, oplog_idx)| PromiseId {
            worker_id,
            oplog_idx: OplogIndex::from_u64(oplog_idx),
        })
    }

    fn interrupt_kind_strat() -> impl Strategy<Value = InterruptKind> {
        prop_oneof! {
            Just(InterruptKind::Interrupt),
            Just(InterruptKind::Restart),
            Just(InterruptKind::Suspend),
        }
    }

    fn shardid_strat() -> impl Strategy<Value = ShardId> {
        any::<i64>().prop_map(ShardId::new)
    }

    fn workererror_strat() -> impl Strategy<Value = WorkerError> {
        prop_oneof! {
            Just(WorkerError::OutOfMemory),
            Just(WorkerError::StackOverflow),
            ".*".prop_map(WorkerError::InvalidRequest),
            ".*".prop_map(WorkerError::Unknown),
        }
    }

    fn golemerror_strat() -> impl Strategy<Value = WorkerExecutorError> {
        prop_oneof! {
            ".*".prop_map(|details| WorkerExecutorError::InvalidRequest { details }),
            workerid_strat().prop_map(|worker_id| WorkerExecutorError::WorkerAlreadyExists { worker_id }),
            workerid_strat().prop_map(|worker_id| WorkerExecutorError::WorkerNotFound { worker_id }),
            (workerid_strat(), ".*").prop_map(|(worker_id, details)| WorkerExecutorError::WorkerCreationFailed { worker_id, details }),
            (workerid_strat(), ".*").prop_map(|(worker_id, reason)| WorkerExecutorError::FailedToResumeWorker { worker_id, reason: Box::new(WorkerExecutorError::unknown(reason)) }),
            (componentid_strat(), any::<u64>(), ".*").prop_map(|(component_id, component_version, reason)| WorkerExecutorError::ComponentDownloadFailed { component_id, component_version, reason }),
            (componentid_strat(), any::<u64>(), ".*").prop_map(|(component_id, component_version, reason)| WorkerExecutorError::ComponentParseFailed { component_id, component_version, reason }),
            (componentid_strat(), ".*").prop_map(|(component_id, reason)| WorkerExecutorError::GetLatestVersionOfComponentFailed { component_id, reason }),
            promiseid_strat().prop_map(|promise_id| WorkerExecutorError::PromiseNotFound { promise_id }),
            promiseid_strat().prop_map(|promise_id| WorkerExecutorError::PromiseDropped { promise_id }),
            promiseid_strat().prop_map(|promise_id| WorkerExecutorError::PromiseAlreadyCompleted { promise_id }),
            promiseid_strat().prop_map(|promise_id| WorkerExecutorError::PromiseAlreadyCompleted { promise_id }),
            interrupt_kind_strat().prop_map(|kind| WorkerExecutorError::Interrupted { kind }),
            ".*".prop_map(|details| WorkerExecutorError::ParamTypeMismatch { details }),
            Just(WorkerExecutorError::NoValueInMessage),
            ".*".prop_map(|details| WorkerExecutorError::ValueMismatch { details }),
            (".*", ".*").prop_map(|(expected, got)| WorkerExecutorError::UnexpectedOplogEntry { expected, got }),
            ".*".prop_map(|details| WorkerExecutorError::Runtime { details }),
            (shardid_strat(), vec(shardid_strat(), 0..100)).prop_map(|(shard_id, shard_ids)| WorkerExecutorError::InvalidShardId { shard_id, shard_ids }),
            Just(WorkerExecutorError::InvalidAccount),
            (workererror_strat(), ".*").prop_map(|(error, stderr)| WorkerExecutorError::PreviousInvocationFailed { error, stderr }),
            Just(WorkerExecutorError::PreviousInvocationExited),
            ".*".prop_map(|details| WorkerExecutorError::Unknown { details }),
            (".*", ".*").prop_map(|(path, reason)| WorkerExecutorError::InitialComponentFileDownloadFailed { path, reason }),
            (".*", ".*").prop_map(|(path, reason)| WorkerExecutorError::FileSystemError { path, reason }),
            (workererror_strat(), ".*").prop_map(|(error, stderr)| WorkerExecutorError::InvocationFailed { error, stderr }),
        }
    }

    fn socketerror_strat() -> impl Strategy<Value = SocketError> {
        prop_oneof! {
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Unknown)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AccessDenied)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotSupported)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidArgument)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::OutOfMemory)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Timeout)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConcurrencyConflict)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotInProgress)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::WouldBlock)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidState)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NewSocketLimit)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressNotBindable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressInUse)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::RemoteUnreachable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionRefused)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionReset)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionAborted)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::DatagramTooLarge)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NameUnresolvable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::TemporaryResolverFailure)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::PermanentResolverFailure)),
            anyhow_strat().prop_map(SocketError::trap),
        }
    }

    fn streamerror_strat() -> impl Strategy<Value = StreamError> {
        prop_oneof! {
            LazyJust::new(|| StreamError::Closed),
            anyhow_strat().prop_map(StreamError::LastOperationFailed),
            anyhow_strat().prop_map(StreamError::Trap)
        }
    }

    fn ipaddress_strat() -> impl Strategy<Value = IpAddress> {
        prop_oneof! {
            (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c, d)| IpAddress::Ipv4((a, b, c, d))),
            (any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>()).prop_map(|(a, b, c, d, e, f, g, h)| IpAddress::Ipv6((a, b, c, d, e, f, g, h))),
        }
    }

    proptest! {
        #[test]
        fn roundtrip_wall_clock_datetime(value in datetime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime = serialized.into();
            prop_assert_eq!(value.seconds, result.seconds);
            prop_assert_eq!(value.nanoseconds, result.nanoseconds);
        }

        #[test]
        fn roundtrip_systemtime(value in systemtime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: SystemTime = serialized.into();
            prop_assert_eq!(value, result);
        }

        #[test]
        fn roundtrip_cap_std_systemtime(value in cap_std_systemtime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: cap_std::time::SystemTime = serialized.into();
            prop_assert_eq!(value, result);
        }

        #[test]
        fn roundtrip_streamerror(value in streamerror_strat()) {
            let serialized: SerializableStreamError = (&value).into();
            let result: StreamError = serialized.into();

            match (value, result) {
                (StreamError::Closed, StreamError::Closed) => (),
                (StreamError::LastOperationFailed(value), StreamError::LastOperationFailed(result)) => {
                    prop_assert_eq!(value.to_string(), result.to_string());
                },
                (StreamError::Trap(value), StreamError::Trap(result)) => {
                    prop_assert_eq!(value.to_string(), result.to_string());
                },
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_ipaddress(value in ipaddress_strat()) {
            let serialized: SerializableIpAddress = value.into();
            let result: IpAddress = serialized.into();

            match (value, result) {
                (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                    prop_assert_eq!(value, result);
                },
                (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                    prop_assert_eq!(value, result);
                },
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_ipaddresses(value in vec(ipaddress_strat(), 0..100)) {
            let serialized: SerializableIpAddresses = value.clone().into();
            let result: Vec<IpAddress> = serialized.into();

            for (value, result) in value.into_iter().zip(result.into_iter()) {
                match (value, result) {
                    (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                        prop_assert_eq!(value, result);
                    },
                    (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                        prop_assert_eq!(value, result);
                    },
                    _ => prop_assert!(false),
                }
            }
        }
    }
}
