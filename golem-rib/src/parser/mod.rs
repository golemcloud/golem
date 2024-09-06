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

mod binary_comparison;
mod boolean;
pub(crate) mod call;
mod cond;
mod flag;
mod identifier;
mod let_binding;
pub(crate) mod literal;
mod multi_line_code_block;
mod not;
mod number;
mod optional;
mod pattern_match;
mod record;
mod result;
pub(crate) mod rib_expr;
mod select_field;
mod select_index;
mod sequence;
mod tuple;
pub(crate) mod type_name;
