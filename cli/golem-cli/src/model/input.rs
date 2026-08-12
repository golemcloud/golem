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

//! Shared CLI input source: a value that is either a filesystem path or STDIN.

use anyhow::{Context, anyhow};
use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Debug)]
pub enum PathBufOrStdin {
    Path(PathBuf),
    Stdin,
}

impl PathBufOrStdin {
    pub fn read_to_string(&self) -> anyhow::Result<String> {
        match self {
            PathBufOrStdin::Path(path) => std::fs::read_to_string(path)
                .with_context(|| anyhow!("Failed to read file: {}", path.display())),
            PathBufOrStdin::Stdin => {
                let mut content = String::new();
                let _ = std::io::stdin()
                    .read_to_string(&mut content)
                    .with_context(|| anyhow!("Failed to read from STDIN"))?;
                Ok(content)
            }
        }
    }

    pub fn is_stdin(&self) -> bool {
        match self {
            PathBufOrStdin::Path(_) => false,
            PathBufOrStdin::Stdin => true,
        }
    }
}

impl FromStr for PathBufOrStdin {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "-" {
            Ok(PathBufOrStdin::Stdin)
        } else {
            Ok(PathBufOrStdin::Path(PathBuf::from_str(s)?))
        }
    }
}
