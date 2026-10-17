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

use crate::model::oplog::payload::HostResponseP3FileSystemWriteAdmission;
use crate::model::oplog::payload::types::{
    SerializableP3FileSystemError, SerializableP3FsErrorCode,
};

super::denied_with_message! {
    permission_check => HostResponsePermissionCheck,
}

pub fn p3_write_admission<R>(
    _request: &R,
    _denial: &str,
) -> HostResponseP3FileSystemWriteAdmission {
    HostResponseP3FileSystemWriteAdmission {
        result: Err(SerializableP3FileSystemError::ErrorCode(
            SerializableP3FsErrorCode::NotPermitted,
        )),
    }
}

pub fn stat<R>(
    _request: &R,
    _denial: &str,
) -> crate::model::oplog::payload::HostResponseFileSystemStat {
    crate::model::oplog::payload::HostResponseFileSystemStat {
        result: Err(
            crate::model::oplog::payload::types::FileSystemError::from_result(Ok(
                wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NotPermitted,
            )),
        ),
    }
}

pub fn p3_stat<R>(
    _request: &R,
    _denial: &str,
) -> crate::model::oplog::payload::HostResponseP3FileSystemStat {
    crate::model::oplog::payload::HostResponseP3FileSystemStat {
        result: Err(SerializableP3FileSystemError::ErrorCode(
            SerializableP3FsErrorCode::NotPermitted,
        )),
    }
}
