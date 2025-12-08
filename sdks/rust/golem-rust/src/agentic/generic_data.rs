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

use crate::Schema;
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Schema)]
pub struct GenericData {
    pub nodes: Vec<GraphNode>,
    pub root: usize,
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

pub trait ToGenericData {
    fn to_generic(&self, graph: &mut GenericData) -> usize;
}

pub trait FromGenericData: Sized {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String>;
}

macro_rules! impl_primitive {
    ($ty:ty, $variant:ident) => {
        impl ToGenericData for $ty {
            fn to_generic(&self, graph: &mut GenericData) -> usize {
                let node = GraphNode::Primitive(PrimitiveNode {
                    value: PrimitiveValue::$variant(*self),
                });
                graph.nodes.push(node);
                graph.nodes.len() - 1
            }
        }

        impl FromGenericData for $ty {
            fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
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

impl ToGenericData for String {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        let node = GraphNode::Primitive(PrimitiveNode {
            value: PrimitiveValue::String(self.clone()),
        });
        graph.nodes.push(node);
        graph.nodes.len() - 1
    }
}

impl FromGenericData for String {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
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

impl<T: ToGenericData> ToGenericData for Option<T> {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        let node = GraphNode::Option(OptionNode {
            some: self.as_ref().map(|v| v.to_generic(graph)),
        });
        graph.nodes.push(node);
        graph.nodes.len() - 1
    }
}

impl<T: FromGenericData> FromGenericData for Option<T> {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Option(OptionNode { some }) => {
                some.map(|i| T::from_generic(graph, i)).transpose()
            }
            _ => Err(format!("Expected Option node at index {}", index)),
        }
    }
}

impl<T: ToGenericData> ToGenericData for Vec<T> {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        let elements = self.iter().map(|v| v.to_generic(graph)).collect();
        let node = GraphNode::Seq(SeqNode { elements });
        graph.nodes.push(node);
        graph.nodes.len() - 1
    }
}

impl<T: FromGenericData> FromGenericData for Vec<T> {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Seq(SeqNode { elements }) => elements
                .iter()
                .map(|&i| T::from_generic(graph, i))
                .collect(),
            _ => Err(format!("Expected Seq node at index {}", index)),
        }
    }
}

impl<K: ToGenericData, V: ToGenericData> ToGenericData for HashMap<K, V> {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        let entries = self
            .iter()
            .map(|(k, v)| (k.to_generic(graph), v.to_generic(graph)))
            .collect();
        let node = GraphNode::Map(MapNode { entries });
        graph.nodes.push(node);
        graph.nodes.len() - 1
    }
}

impl<K: FromGenericData + Eq + Hash, V: FromGenericData> FromGenericData for HashMap<K, V> {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
        match &graph.nodes[index] {
            GraphNode::Map(MapNode { entries }) => {
                let mut map = HashMap::new();
                for (ki, vi) in entries {
                    map.insert(K::from_generic(graph, *ki)?, V::from_generic(graph, *vi)?);
                }
                Ok(map)
            }
            _ => Err(format!("Expected Map node at index {}", index)),
        }
    }
}

impl<T: ToGenericData> ToGenericData for Box<T> {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        (**self).to_generic(graph)
    }
}

impl<T: FromGenericData> FromGenericData for Box<T> {
    fn from_generic(graph: &GenericData, index: usize) -> Result<Self, String> {
        Ok(Box::new(T::from_generic(graph, index)?))
    }
}

impl<T: ToGenericData> ToGenericData for &T {
    fn to_generic(&self, graph: &mut GenericData) -> usize {
        (*self).to_generic(graph)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use test_r::test;

    use crate::agentic::{FromGenericData, GenericData, Schema, ToGenericData};
    use crate::Schema;

    #[derive(Schema, Debug, Clone, PartialEq)]
    enum MyEnum {
        A(String),
        B(Box<MyEnum>),
        C(Box<Option<MyEnum>>),
        D(Vec<MyEnum>),
        E(HashMap<String, MyEnum>),
    }

    #[test]
    fn test_graph_serialization() {
        let value = MyEnum::C(Box::new(Some(MyEnum::A("foo".to_string()))));

        let mut graph = GenericData {
            nodes: vec![],
            root: 0,
        };
        let root_index = value.to_generic(&mut graph);
        graph.root = root_index;

        let deserialized: MyEnum = MyEnum::from_generic(&graph, graph.root).unwrap();

        assert_eq!(value, deserialized);
    }

    #[test]
    fn test_schema_instance_for_recursive_types() {
        // primitive
        let value = MyEnum::A("foo!".to_string());
        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());
        assert_eq!(result, Ok(value.clone()));

        // nesting
        let value = MyEnum::B(Box::new(MyEnum::A("bar".to_string())));
        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());

        assert_eq!(result, Ok(value.clone()));

        // complex nesting
        let value = MyEnum::B(Box::new(MyEnum::B(Box::new(MyEnum::B(Box::new(
            MyEnum::A("Hello, Graph!".to_string()),
        ))))));
        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());

        assert_eq!(result, Ok(value.clone()));

        // option
        let value = MyEnum::C(Box::new(Some(MyEnum::A("foo".to_string()))));
        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());

        assert_eq!(result, Ok(value));

        // vec
        let value = MyEnum::D(vec![
            MyEnum::A("one".to_string()),
            MyEnum::B(Box::new(MyEnum::A("two".to_string()))),
            MyEnum::C(Box::new(None)),
        ]);

        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());

        assert_eq!(result, Ok(value));

        // hashmap
        let mut map = HashMap::new();
        map.insert("first".to_string(), MyEnum::A("uno".to_string()));
        map.insert(
            "second".to_string(),
            MyEnum::B(Box::new(MyEnum::A("dos".to_string()))),
        );
        let value = MyEnum::E(map);
        let result: Result<MyEnum, String> =
            Schema::from_wit_value(value.clone().to_wit_value().unwrap(), MyEnum::get_type());
        assert_eq!(result, Ok(value));
    }
}
