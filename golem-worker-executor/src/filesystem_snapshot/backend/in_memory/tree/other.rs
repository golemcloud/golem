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

use std::{io, path::Path};

pub(super) fn check_symlink_support() -> io::Result<()> {
    Err(symlinks_unsupported())
}

pub(super) fn create_symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(symlinks_unsupported())
}

fn symlinks_unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the in-memory snapshot store cannot make symlinks on this platform",
    )
}
