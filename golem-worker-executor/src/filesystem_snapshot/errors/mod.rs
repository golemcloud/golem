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

//! Operation results and their error chains.

use super::run_admission::Withdrawal;
use std::fmt::{Display, Formatter};

/// The store could not finish a call. A later call can succeed. The doc of each method names the
/// causes, and says what a failed call can leave.
#[derive(Debug)]
pub(crate) struct Failed(anyhow::Error);

impl Failed {
    /// Gives the failure with its cause.
    pub(crate) fn new(cause: anyhow::Error) -> Self {
        Self(cause)
    }

    /// Gives the cause of the failure.
    #[cfg(test)]
    pub(crate) fn cause(&self) -> &anyhow::Error {
        &self.0
    }
}

impl Display for Failed {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the filesystem snapshot store could not finish the call: {:#}",
            self.0
        )
    }
}

impl std::error::Error for Failed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Why a call that only the limiter or the storage can end did not succeed.
#[derive(Debug)]
pub(crate) enum CallError {
    /// The limiter withdrew the call, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a save did not succeed.
#[derive(Debug)]
pub(crate) enum SaveError {
    /// Another snapshot of the agent has the name.
    NameInUse,
    /// The save could not read the tree.
    Source(std::io::Error),
    /// The limiter withdrew the call, the cancel of the save fired, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a restore did not succeed.
#[derive(Debug)]
pub(crate) enum RestoreFailure {
    /// No complete snapshot has the name.
    NotFound,
    /// An integrity check failed.
    Corrupt(anyhow::Error),
    /// The directory could not take the tree.
    Destination(std::io::Error),
    /// The limiter withdrew the call, or the store is shut down.
    Stopped(Withdrawal),
    Failed(Failed),
}

/// Why a read without a limiter did not succeed.
#[derive(Debug)]
pub(crate) enum ReadError {
    /// An integrity check failed.
    Corrupt(anyhow::Error),
    /// The store is shut down.
    Stopped,
    Failed(Failed),
}

impl Display for CallError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for CallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Stopped(_) => None,
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for SaveError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameInUse => formatter.write_str("a filesystem snapshot already has the name"),
            Self::Source(error) => write!(
                formatter,
                "failed to read the tree of the filesystem snapshot: {error}"
            ),
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NameInUse | Self::Stopped(_) => None,
            Self::Source(error) => Some(error),
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for RestoreFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no complete filesystem snapshot has the name"),
            Self::Corrupt(source) => write!(
                formatter,
                "a filesystem snapshot failed an integrity check: {source:#}"
            ),
            Self::Destination(error) => write!(
                formatter,
                "failed to write the tree of the filesystem snapshot: {error}"
            ),
            Self::Stopped(cause) => write!(formatter, "{cause}"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for RestoreFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound | Self::Stopped(_) => None,
            Self::Corrupt(source) => Some(source.as_ref()),
            Self::Destination(error) => Some(error),
            Self::Failed(failed) => Some(failed),
        }
    }
}

impl Display for ReadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt(source) => write!(
                formatter,
                "a filesystem snapshot failed an integrity check: {source:#}"
            ),
            Self::Stopped => formatter.write_str("the filesystem snapshot store is shut down"),
            Self::Failed(failed) => write!(formatter, "{failed}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Corrupt(source) => Some(source.as_ref()),
            Self::Stopped => None,
            Self::Failed(failed) => Some(failed),
        }
    }
}

#[cfg(test)]
mod tests;
