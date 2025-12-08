// Copyright 2024-2025 Golem Cloud
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

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use crate::Schema;

#[derive(Debug, Clone, Schema)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub root: usize
}

#[derive(Debug, Clone, Schema)]
pub enum GraphNode {
    Primitive(PrimitiveNode),
    Struct(StructNode),
    Enum(EnumNode),
    Seq(SeqNode),
    Option(OptionNode),
    Map(MapNode),
    Set(SetNode),
}

#[derive(Debug, Clone, Schema)]
pub enum PrimitiveValue {
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Bool(bool),
    String(String),
}

#[derive(Debug, Clone, Schema)]
pub struct PrimitiveNode {
    pub value: PrimitiveValue,
}

#[derive(Debug, Clone, Schema)]
pub struct StructNode {
    pub fields: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Schema)]
pub struct EnumNode {
    pub variant: String,
    pub payload: Option<usize>,
}

#[derive(Debug, Clone, Schema)]
pub struct SeqNode {
    pub elements: Vec<usize>,
}

#[derive(Debug, Clone, Schema)]
pub struct OptionNode {
    pub some: Option<usize>,
}

#[derive(Debug, Clone, Schema)]
pub struct MapNode {
    pub entries: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, Schema)]
pub struct SetNode {
    pub elements: Vec<usize>,
}

pub trait ToGraph {
    fn to_graph(&self, graph: &mut Graph) -> usize;
}

pub trait FromGraph: Sized {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String>;
}

macro_rules! impl_primitive {
    ($ty:ty, $variant:ident) => {
        impl ToGraph for $ty {
            fn to_graph(&self, graph: &mut Graph) -> usize {
                let index = graph.nodes.len();
                let node = GraphNode::Primitive(PrimitiveNode {
                    value: PrimitiveValue::$variant(*self),
                });
                graph.nodes.push(node);
                index
            }
        }

        impl FromGraph for $ty {
            fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
                match &graph.nodes[index] {
                    GraphNode::Primitive(PrimitiveNode { value }) => {
                        if let PrimitiveValue::$variant(v) = value {
                            Ok(*v)
                        } else {
                            Err(format!("Expected {}, found {:?}", stringify!($ty), value))
                        }
                    }
                    _ => Err(format!("Expected primitive node at index {}", index)),
                }
            }
        }
    };
}

impl_primitive!(i32, I32);
impl_primitive!(u32, U32);
impl_primitive!(i64, I64);
impl_primitive!(u64, U64);
impl_primitive!(f32, F32);
impl_primitive!(f64, F64);
impl_primitive!(bool, Bool);

impl ToGraph for String {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        let index = graph.nodes.len();
        let node = GraphNode::Primitive(PrimitiveNode {
            value: PrimitiveValue::String(self.clone()),
        });
        graph.nodes.push(node);
        index
    }
}

impl FromGraph for String {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Primitive(PrimitiveNode { value }) => {
                if let PrimitiveValue::String(v) = value {
                    Ok(v.clone())
                } else {
                    Err(format!("Expected String, found {:?}", value))
                }
            }
            _ => Err(format!("Expected primitive node at index {}", index)),
        }
    }
}

impl<T: ToGraph> ToGraph for Option<T> {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        let index = graph.nodes.len();
        let node = GraphNode::Option(OptionNode {
            some: self.as_ref().map(|v| v.to_graph(graph)),
        });
        graph.nodes.push(node);
        graph.nodes.len() - 1
    }
}

impl<T: FromGraph> FromGraph for Option<T> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Option(OptionNode { some }) => {
                some.map(|i| T::from_graph(graph, i)).transpose()
            }
            _ => Err(format!("Expected Option node at index {}", index)),
        }
    }
}

impl<T: ToGraph> ToGraph for Vec<T> {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        let index = graph.nodes.len();
        let elements = self.iter().map(|v| v.to_graph(graph)).collect();
        let node = GraphNode::Seq(SeqNode { elements });
        graph.nodes.push(node);
        index
    }
}

impl<T: FromGraph> FromGraph for Vec<T> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Seq(SeqNode { elements }) => {
                elements.iter().map(|&i| T::from_graph(graph, i)).collect()
            }
            _ => Err(format!("Expected Seq node at index {}", index)),
        }
    }
}

impl<K: ToGraph, V: ToGraph> ToGraph for HashMap<K, V> {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        let index = graph.nodes.len();
        let entries = self.iter().map(|(k, v)| (k.to_graph(graph), v.to_graph(graph))).collect();
        let node = GraphNode::Map(MapNode { entries });
        graph.nodes.push(node);
        index
    }
}

impl<K: FromGraph + Eq + Hash, V: FromGraph> FromGraph for HashMap<K, V> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Map(MapNode { entries }) => {
                let mut map = HashMap::new();
                for (ki, vi) in entries {
                    map.insert(K::from_graph(graph, *ki)?, V::from_graph(graph, *vi)?);
                }
                Ok(map)
            }
            _ => Err(format!("Expected Map node at index {}", index)),
        }
    }
}

impl<T: ToGraph> ToGraph for HashSet<T> {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        let index = graph.nodes.len();
        let elements = self.iter().map(|v| v.to_graph(graph)).collect();
        let node = GraphNode::Set(SetNode { elements });
        graph.nodes.push(node);
        index
    }
}

impl<T: FromGraph + Eq + Hash> FromGraph for HashSet<T> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Set(SetNode { elements }) => {
                let mut set = HashSet::new();
                for &i in elements {
                    set.insert(T::from_graph(graph, i)?);
                }
                Ok(set)
            }
            _ => Err(format!("Expected Set node at index {}", index)),
        }
    }
}

impl<T: ToGraph> ToGraph for Box<T> {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        (**self).to_graph(graph)
    }
}

impl<T: FromGraph> FromGraph for Box<T> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        Ok(Box::new(T::from_graph(graph, index)?))
    }
}

impl<T: ToGraph> ToGraph for &T {
    fn to_graph(&self, graph: &mut Graph) -> usize {
        (*self).to_graph(graph)
    }
}


#[cfg(test)]
mod tests {
    use golem_wasm::WitValue;
    use test_r::test;

    use crate::agentic::{FromGraph, Schema, ToGraph};
    use crate::Schema;

    #[derive(Debug)]
    enum MyEnum {
        A(String),
        C(Box<MyEnum>),
        D(Box<Option<MyEnum>>)
    }

    impl crate::agentic::ToGraph for MyEnum {
        fn to_graph(&self, graph: &mut crate::agentic::Graph) -> usize {
            match self {
                Self::A(f0) => {
                    let child_idx = crate::agentic::ToGraph::to_graph(&f0, graph);
                    let index = graph.nodes.len();
                    graph.nodes.push(crate::agentic::GraphNode::Enum(crate::agentic::EnumNode { variant: "A".to_string(), payload: Some(child_idx) }));
                    index
                }
                Self::C(f0) => {
                    let child_idx = crate::agentic::ToGraph::to_graph(&f0, graph);
                    let index = graph.nodes.len();
                    graph.nodes.push(crate::agentic::GraphNode::Enum(crate::agentic::EnumNode { variant: "C".to_string(), payload: Some(child_idx) }));
                    index
                }
                Self::D(f0)
                => {
                    let child_idx = crate::agentic::ToGraph::to_graph(&f0, graph);
                    let index = graph.nodes.len();
                    graph.nodes.push(crate::agentic::GraphNode::Enum(crate::agentic::EnumNode { variant: "D".to_string(), payload: Some(child_idx) }));
                    index
                }
            }
        }
    }
    impl crate::agentic::FromGraph for MyEnum {
        fn from_graph(graph: &crate::agentic::Graph, index: usize) -> Result<Self, String> {
            match &graph.nodes[index] {
                crate::agentic::GraphNode::Enum(enum_node) => {
                    let payload_index = enum_node.payload.ok_or("Missing payload")?;
                    match enum_node.variant.as_str() {
                        "A" => Ok(Self::A(crate::agentic::FromGraph::from_graph(graph, payload_index)?)),
                        "C" => Ok(Self::C(crate::agentic::FromGraph::from_graph(graph, payload_index)?)),
                        "D" => Ok(Self::D(crate::agentic::FromGraph::from_graph(graph, payload_index)?)
                        ),
                        other => Err(format!("Unknown enum variant: {}", other)),
                    }
                }
                _ => Err(format!("Expected Enum node at index {}", index)),
            }
        }
    }


    #[test]
    fn test_graph_serialization() {

        // let value =  MyEnum::C(Box::new(MyEnum::C(Box::new(MyEnum::C(Box::new(MyEnum::A("Hello, Graph!".to_string())))))));
        let value =  MyEnum::D(Box::new(Some(MyEnum::A("Hello, Graph!".to_string()))));
       // let result: Result<MyEnum, String> = Schema::from_wit_value(value.to_wit_value().unwrap(), MyEnum::get_type());

       // dbg!(&result);

        let mut graph = crate::agentic::Graph { nodes: vec![], root: 0 };

        let result = value.to_graph(
            &mut graph
        );

        dbg!(&result);
        //
         graph.root = result;
        //
        dbg!(&graph);
        //
        let result = MyEnum::from_graph(&graph, graph.root);
        //
        dbg!(&result);


        assert!(false)
    }
}