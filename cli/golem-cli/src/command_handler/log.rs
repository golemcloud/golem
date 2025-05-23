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

use crate::context::Context;
use crate::model::text::fmt::{NestedTextViewIndent, TextView};
use crate::model::Format;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

pub struct LogHandler {
    ctx: Arc<Context>,
}

impl LogHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn log_view<View: TextView + Serialize + DeserializeOwned>(&self, view: &View) {
        match self.ctx.format() {
            Format::Json => {
                println!("{}", serde_json::to_string(view).unwrap());
            }
            Format::Yaml => {
                // TODO: handle "streaming" optionally
                println!("---\n{}", serde_yaml::to_string(view).unwrap());
            }
            Format::Text => {
                view.log();
            }
        }
    }

    pub fn nested_text_view_indent(&self) -> NestedTextViewIndent {
        NestedTextViewIndent::new(self.ctx.format())
    }
}
