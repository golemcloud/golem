// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::printer::{PrintContext, TreePrinter};

pub struct StringContext {
    ctx: String,
}

impl StringContext {
    pub fn new() -> StringContext {
        StringContext { ctx: String::new() }
    }

    pub fn print_to_string(mut self, p: TreePrinter<StringContext>) -> String {
        p.print(&mut self);

        self.ctx
    }
}

impl PrintContext for StringContext {
    fn print_str(&mut self, s: &str) {
        self.ctx.push_str(s)
    }
}

pub fn unit() -> TreePrinter<StringContext> {
    TreePrinter::unit()
}
