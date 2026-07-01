// Copyright 2024-2026 Golem Cloud
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

use crate::agentic::extended_tool_type::ExtendedToolType;
use crate::golem_agentic::exports::golem::tool::guest::Tool;
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct State {
    pub tools: RefCell<Tools>,
}
#[derive(Default)]
pub struct Tools {
    pub tools: BTreeMap<String, ExtendedToolType>,
}
static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
pub fn get_state() -> &'static State {
    unsafe {
        if STATE.is_none() {
            STATE = Some(State::default());
        }
        STATE.as_ref().unwrap()
    }
}

pub fn register_tool(tool: ExtendedToolType) {
    tool.try_to_tool().expect("tool descriptor build failed");
    let name = tool.tool_name().to_string();
    let state = get_state();
    let mut tools = state.tools.borrow_mut();
    if tools.tools.contains_key(&name) {
        panic!("duplicate tool registration for tool name: {name}");
    }
    tools.tools.insert(name, tool);
}

pub fn get_all_tools() -> Vec<Tool> {
    get_state()
        .tools
        .borrow()
        .tools
        .values()
        .map(|t| t.to_tool())
        .collect()
}
pub fn get_tool_by_name(name: &str) -> Option<Tool> {
    get_extended_tool_by_name(name).map(|t| t.to_tool())
}
pub fn get_extended_tool_by_name(name: &str) -> Option<ExtendedToolType> {
    get_state().tools.borrow().tools.get(name).cloned()
}

#[cfg(test)]
pub(crate) fn clear_tools_for_tests() {
    get_state().tools.borrow_mut().tools.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic::{Doc, ExtendedCommandNode, ExtendedGlobals, ExtendedToolType};
    use test_r::test;

    fn tool(name: &str) -> ExtendedToolType {
        ExtendedToolType {
            version: "0.1.0".into(),
            commands: vec![ExtendedCommandNode {
                name: name.into(),
                aliases: vec![],
                doc: Doc {
                    summary: String::new(),
                    description: String::new(),
                    examples: vec![],
                },
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: None,
            }],
        }
    }

    #[test]
    #[should_panic]
    fn duplicate_registration_panics() {
        clear_tools_for_tests();
        register_tool(tool("dupe"));
        register_tool(tool("dupe"));
    }
}
