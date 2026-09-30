pub use golem_schema::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};

pub mod extended_tool_type;
pub mod reflected_descriptor;
pub mod tool_literal;
pub mod tool_refinement;

#[cfg(feature = "guest")]
pub mod wire_descriptor;
#[cfg(feature = "guest")]
pub use wire_descriptor::*;

pub use extended_tool_type::*;
pub use golem_schema::schema::tool::{
    BoolFlagShape, DuplicateKeyPolicy, ErrorKind, FlagShape, Quantifier, Repetition,
};
pub use tool_literal::*;
pub use tool_refinement::*;

#[cfg(test)]
test_r::enable!();
