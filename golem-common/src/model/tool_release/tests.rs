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

use super::tool_metadata_digest;
use crate::model::tool::HostToolId;
use crate::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
use golem_schema::schema::SchemaGraph;
use test_r::test;

fn tool(name: &str, version: &str) -> Tool {
    Tool {
        requires_filesystem: false,
        version: version.to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: name.to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

#[test]
fn metadata_digest_is_deterministic_and_covers_schema_version_and_definition() {
    let definition = tool("search", "1.0.0");
    let digest = tool_metadata_digest("0.1.0", &definition).unwrap();

    assert_eq!(digest, tool_metadata_digest("0.1.0", &definition).unwrap());
    assert_ne!(digest, tool_metadata_digest("0.2.0", &definition).unwrap());
    assert_ne!(
        digest,
        tool_metadata_digest("0.1.0", &tool("search", "1.1.0")).unwrap()
    );
}

#[test]
fn host_tool_id_requires_lower_kebab_case() {
    assert_eq!(
        HostToolId::try_from("golem-search".to_string())
            .unwrap()
            .as_str(),
        "golem-search"
    );
    assert!(HostToolId::try_from(String::new()).is_err());
    assert!(HostToolId::try_from("GolemSearch".to_string()).is_err());
    assert!(HostToolId::try_from("golem search".to_string()).is_err());
}
