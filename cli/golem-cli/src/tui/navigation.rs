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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Workspace {
    Dev,
    Ops,
}

impl Workspace {
    pub(super) const ALL: [Self; 2] = [Self::Dev, Self::Ops];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Dev => "Dev",
            Self::Ops => "Ops",
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    pub(super) fn from_index(index: usize) -> Self {
        Self::ALL[index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DevView {
    BuildDeploy,
    Server,
    Repl,
}

impl DevView {
    pub(super) const ALL: [Self; 3] = [Self::BuildDeploy, Self::Server, Self::Repl];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::BuildDeploy => "Build & Deploy",
            Self::Repl => "REPL",
            Self::Server => "Server",
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }
}
