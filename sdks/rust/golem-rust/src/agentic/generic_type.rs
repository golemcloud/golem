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
    pub fields: Vec<(String, usize)>, // name + index of child node
}

#[derive(Debug, Clone, Schema)]
pub struct EnumNode {
    pub variant: String,
    pub payload: Option<usize>, // Some(index of child) or None
}

#[derive(Debug, Clone, Schema)]
pub struct SeqNode {
    pub elements: Vec<usize>, // indexes of child nodes
}

#[derive(Debug, Clone, Schema)]
pub struct OptionNode {
    pub some: Option<usize>, // None => None, Some(index) => Some(value)
}

#[derive(Debug, Clone, Schema)]
pub struct MapNode {
    pub entries: Vec<(usize, usize)>, // Vec of (key_index, value_index)
}

#[derive(Debug, Clone, Schema)]
pub struct SetNode {
    pub elements: Vec<usize>, // indexes of values
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
        index
    }
}

impl<T: FromGraph> FromGraph for Option<T> {
    fn from_graph(graph: &Graph, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Option(OptionNode { some }) => {
                Ok(some.map(|i| T::from_graph(graph, i)).transpose().unwrap())
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
