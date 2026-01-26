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

use crate::bridge_gen::type_naming::type_location::{
    TypeLocation, TypeLocationPath, TypeLocationRoot,
};

pub(super) struct Builder {
    root_owner: RootOwner,
    root_item_name: String,
    path: Option<TypeLocationPath>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            root_owner: RootOwner::ConstructorInput,
            root_item_name: "".to_string(),
            path: None,
        }
    }

    pub fn set_root_owner(&mut self, owner: RootOwner) {
        self.root_owner = owner;
        self.root_item_name = "".to_string();
        self.path = None;
    }

    pub fn set_root_item_name(&mut self, name: &str) {
        self.root_item_name = name.to_string();
        self.path = None;
    }

    pub fn push(&mut self, elem: TypeLocationPath) {
        match &mut self.path {
            None => {
                self.path = Some(elem);
            }
            Some(path) => Self::push_path(path.inner_mut(), elem),
        }
    }

    fn push_path(path: &mut Option<Box<TypeLocationPath>>, elem: TypeLocationPath) {
        match path {
            Some(path) => {
                Self::push_path(path.inner_mut(), elem);
            }
            None => {
                *path = Some(Box::new(elem));
            }
        }
    }

    pub fn pop(&mut self) {
        if let Some(path) = &mut self.path {
            if !Self::pop_path(path) {
                self.path = None;
            }
        }
    }

    fn pop_path(path: &mut TypeLocationPath) -> bool {
        enum InnerPopResult {
            NoInner,
            Popped,
            NotPopped,
        }

        let inner_pop_result = match path.inner_mut() {
            Some(inner) => {
                if Self::pop_path(inner) {
                    InnerPopResult::Popped
                } else {
                    InnerPopResult::NotPopped
                }
            }
            None => InnerPopResult::NoInner,
        };

        match inner_pop_result {
            InnerPopResult::NoInner => false,
            InnerPopResult::Popped => true,
            InnerPopResult::NotPopped => {
                *path.inner_mut() = None;
                true
            }
        }
    }

    pub fn type_location(&self) -> TypeLocation {
        TypeLocation {
            root: match &self.root_owner {
                RootOwner::ConstructorInput => TypeLocationRoot::AgentConstructorInput {
                    input_name: self.root_item_name.clone(),
                },
                RootOwner::MethodInput { method_name } => TypeLocationRoot::AgentMethodInput {
                    method_name: method_name.clone(),
                    input_name: self.root_item_name.clone(),
                },
                RootOwner::MethodOutput { method_name } => TypeLocationRoot::AgentMethodOutput {
                    method_name: method_name.clone(),
                    output_name: self.root_item_name.clone(),
                },
            },
            path: self.path.clone(),
        }
    }
}

pub(super) enum RootOwner {
    ConstructorInput,
    MethodInput { method_name: String },
    MethodOutput { method_name: String },
}
