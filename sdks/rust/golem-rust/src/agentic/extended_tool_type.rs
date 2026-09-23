pub use golem_tool_metadata::extended_tool_type::*;

use crate::agentic::Schema;
use crate::schema::SchemaGraph;
use crate::schema::tool::wit::wire;
use crate::schema::wit::direct::{self, FromWire, IntoWire, WireReader};
use std::collections::{HashMap, HashSet};

pub type Tool = wire::Tool;

pub fn tool_value_schema<T: Schema>(position: &str) -> Result<SchemaGraph, ToolBuildError> {
    T::get_type()
        .get_schema_graph()
        .ok_or_else(|| ToolBuildError::AutoInjectedToolParameter(position.to_string()))
}

pub fn encode_schema_value_default(
    value: &crate::SchemaValue,
) -> Result<crate::schema::wit::wire::SchemaValueTree, ToolBuildError> {
    crate::schema::wit::encode_value(value)
        .map_err(|error| ToolBuildError::EncodeError(error.to_string()))
}

#[doc(hidden)]
pub struct DirectToolInput {
    reader: WireReader,
    fields: HashMap<String, crate::schema::wit::wire::ValueNodeIndex>,
}

impl DirectToolInput {
    fn record_root(graph: &crate::schema::wit::wire::SchemaGraph) -> Result<usize, String> {
        use crate::schema::wit::wire::SchemaTypeBody;
        let mut root = graph.root;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(root) {
                return Err("tool input schema root contains a reference cycle".to_string());
            }
            let node = graph
                .type_nodes
                .get(root as usize)
                .ok_or("tool input schema root is out of bounds")?;
            match &node.body {
                SchemaTypeBody::RecordType(_) => return Ok(root as usize),
                SchemaTypeBody::RefType(index) => {
                    root = graph
                        .defs
                        .get(*index as usize)
                        .ok_or("tool input schema definition is out of bounds")?
                        .body;
                }
                _ => return Err("tool input schema root is not a record".to_string()),
            }
        }
    }

    pub fn add_root_aliases(
        graph: &mut crate::schema::wit::wire::SchemaGraph,
        name: &str,
        aliases: &[&str],
    ) -> Result<(), String> {
        let root = Self::record_root(graph)?;
        if let crate::schema::wit::wire::SchemaTypeBody::RecordType(fields) =
            &mut graph.type_nodes[root].body
        {
            for field in fields.iter_mut().filter(|field| field.name == name) {
                field
                    .metadata
                    .aliases
                    .extend(aliases.iter().map(|alias| (*alias).to_string()));
            }
        }
        Ok(())
    }

    pub fn new(value: crate::schema::wit::wire::TypedSchemaValue) -> Result<Self, String> {
        use crate::schema::wit::wire::{SchemaTypeBody, SchemaValueNode};
        let root = Self::record_root(&value.graph)?;
        let SchemaTypeBody::RecordType(field_types) = &value.graph.type_nodes[root].body else {
            unreachable!("record_root only returns record indices");
        };
        let mut reader = WireReader::new(value.value.value_nodes);
        let SchemaValueNode::RecordValue(indices) = reader
            .take(value.value.root)
            .map_err(|error| error.to_string())?
        else {
            return Err("tool input value root is not a record".to_string());
        };
        if field_types.len() != indices.len() {
            return Err("tool input record field count does not match its flat schema".to_string());
        }
        let mut fields = HashMap::new();
        let mut field_indices = HashSet::new();
        for (field, index) in field_types.iter().zip(indices) {
            if !field_indices.insert(index) {
                return Err("tool input record references a field more than once".to_string());
            }
            for name in std::iter::once(&field.name).chain(&field.metadata.aliases) {
                if let Some(previous) = fields.insert(name.clone(), index)
                    && previous != index
                {
                    return Err(format!("ambiguous canonical tool input field `{name}`"));
                }
            }
        }
        Ok(Self { reader, fields })
    }

    pub fn take<T: FromWire>(&mut self, name: &str) -> Result<T, String> {
        let index = self
            .fields
            .remove(name)
            .ok_or_else(|| format!("missing canonical tool input field `{name}`"))?;
        self.fields.retain(|_, candidate| *candidate != index);
        T::read_wire(&mut self.reader, index).map_err(|error| error.to_string())
    }

    pub fn take_any<T: FromWire>(&mut self, names: &[&str]) -> Result<T, String> {
        let name = names
            .iter()
            .copied()
            .find(|name| self.fields.contains_key(*name))
            .ok_or_else(|| format!("missing canonical tool input field `{}`", names[0]))?;
        self.take(name)
    }

    pub fn finish(mut self) -> Result<(), String> {
        for index in self.fields.into_values().collect::<HashSet<_>>() {
            self.reader
                .discard(index)
                .map_err(|error| error.to_string())?;
        }
        self.reader.finish().map_err(|error| error.to_string())
    }
}

#[doc(hidden)]
pub async fn encode_direct_tool_value<T: IntoWire + direct::WireSchema + ?Sized>(
    value: &T,
) -> Result<crate::schema::wit::wire::TypedSchemaValue, String> {
    Ok(crate::schema::wit::wire::TypedSchemaValue {
        graph: direct::schema::<T>(),
        value: direct::encode_async(value)
            .await
            .map_err(|error| error.to_string())?,
    })
}

#[allow(async_fn_in_trait)]
#[doc(hidden)]
pub trait DirectToolError {
    async fn direct_error_payload(
        &self,
    ) -> Result<(String, crate::schema::wit::wire::TypedSchemaValue), String>;
}
