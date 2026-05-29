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

use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::render::walker::{SchemaWalker, WalkerError, walk};
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use test_r::test;

struct CountingWalker {
    count: usize,
}

impl SchemaWalker for CountingWalker {
    type Output = ();
    type Error = std::convert::Infallible;

    fn walk(
        &mut self,
        _graph: &SchemaGraph,
        _ty: &SchemaType,
        _value: &SchemaValue,
    ) -> Result<(), Self::Error> {
        self.count += 1;
        Ok(())
    }
}

#[test]
fn walker_resolves_ref_before_dispatch() {
    let id = TypeId::new("Inner");
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: id.clone(),
            name: None,
            metadata: Default::default(),
            body: SchemaType::U32,
        }],
        root: SchemaType::Ref(id.clone()),
    };
    let mut walker = CountingWalker { count: 0 };
    let res = walk(&mut walker, &graph, &graph.root, &SchemaValue::U32(7));
    assert!(res.is_ok());
    assert_eq!(walker.count, 1);
}

#[test]
fn walker_detects_ref_cycle() {
    let a = TypeId::new("A");
    let b = TypeId::new("B");
    let graph = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: a.clone(),
                name: None,
                metadata: Default::default(),
                body: SchemaType::Ref(b.clone()),
            },
            SchemaTypeDef {
                id: b.clone(),
                name: None,
                metadata: Default::default(),
                body: SchemaType::Ref(a.clone()),
            },
        ],
        root: SchemaType::Ref(a),
    };
    let mut walker = CountingWalker { count: 0 };
    let res = walk(&mut walker, &graph, &graph.root, &SchemaValue::U32(0));
    assert!(matches!(res, Err(WalkerError::RefCycle(_))));
}

#[test]
fn walker_reports_dangling_ref() {
    let graph = SchemaGraph {
        defs: vec![],
        root: SchemaType::Ref(TypeId::new("missing")),
    };
    let mut walker = CountingWalker { count: 0 };
    let res = walk(&mut walker, &graph, &graph.root, &SchemaValue::U32(0));
    assert!(matches!(res, Err(WalkerError::DanglingRef(_))));
}
