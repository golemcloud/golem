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

use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path(Vec<PathElem>);

impl Path {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn current(&self) -> Option<&PathElem> {
        self.0.first()
    }

    pub fn progress(&mut self) {
        if !self.0.is_empty() {
            self.0.remove(0);
        }
    }

    pub fn from_elem(elem: PathElem) -> Self {
        Path(vec![elem])
    }

    pub fn from_elems(elems: Vec<&str>) -> Self {
        Path(
            elems
                .iter()
                .map(|x| PathElem::Field(x.to_string()))
                .collect(),
        )
    }

    pub fn push_front(&mut self, elem: PathElem) {
        self.0.insert(0, elem);
    }

    pub fn push_back(&mut self, elem: PathElem) {
        self.0.push(elem);
    }
}

pub enum PathType {
    RecordPath(Path),
    IndexPath(Path),
}

impl PathType {
    pub fn from_path(path: &Path) -> Option<PathType> {
        if path.0.first().map(|elem| elem.is_field()).unwrap_or(false) {
            Some(PathType::RecordPath(path.clone()))
        } else if path.0.first().map(|elem| elem.is_index()).unwrap_or(false) {
            Some(PathType::IndexPath(path.clone()))
        } else {
            None
        }
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut is_first = true;

        for elem in &self.0 {
            match elem {
                PathElem::Field(name) => {
                    if is_first {
                        write!(f, "{}", name)?;
                        is_first = false;
                    } else {
                        write!(f, ".{}", name)?;
                    }
                }
                PathElem::Index(index) => {
                    if is_first {
                        write!(f, "index: {}", index)?;
                        is_first = false;
                    } else {
                        write!(f, "[{}]", index)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathElem {
    Field(String),
    Index(usize),
}

impl PathElem {
    pub fn is_field(&self) -> bool {
        matches!(self, PathElem::Field(_))
    }

    pub fn is_index(&self) -> bool {
        matches!(self, PathElem::Index(_))
    }
}
