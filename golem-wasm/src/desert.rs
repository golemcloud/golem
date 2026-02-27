use crate::golem_core_1_5_x::types::NamedWitTypeNode;
use crate::{ResourceMode, Uri, Value, WitNode, WitType, WitTypeNode, WitValue};
use desert_rust::adt::{AdtMetadata, AdtSerializer};
use desert_rust::{
    deserialize_iterator, serialize_iterator, BinaryDeserializer, BinaryInput, BinaryOutput,
    BinarySerializer, DeserializationContext, Evolution, SerializationContext,
};
use lazy_static::lazy_static;
use std::borrow::Cow;

pub struct VecValueWrapper<'a>(pub Cow<'a, [Value]>);

impl<'a> BinarySerializer for VecValueWrapper<'a> {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let all_u8 = self.0.iter().all(|v| matches!(v, Value::U8(_)));
        if all_u8 {
            context.write_u8(1); // special case 1
            let bytes = self
                .0
                .iter()
                .map(|v| match v {
                    Value::U8(b) => *b,
                    _ => unreachable!(),
                })
                .collect::<Vec<u8>>();
            context.write_var_u32(bytes.len() as u32);
            context.write_bytes(&bytes);
            Ok(())
        } else {
            context.write_u8(0); // default case 0
            serialize_iterator(&mut self.0.iter(), context)
        }
    }
}

impl<'a> BinaryDeserializer for VecValueWrapper<'a> {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        match tag {
            0 => {
                let (iter, maybe_size) = deserialize_iterator(context);
                let mut vec = Vec::with_capacity(maybe_size.unwrap_or_default());
                for item in iter {
                    vec.push(item?);
                }

                Ok(Self(Cow::Owned(vec)))
            }
            1 => {
                let length = context.read_var_u32()? as usize;
                let bytes = context.read_bytes(length)?;
                Ok(Self(Cow::Owned(
                    bytes.iter().map(|b| Value::U8(*b)).collect(),
                )))
            }
            other => Err(desert_rust::Error::DeserializationFailure(format!(
                "Invalid Vec<Value> tag: {}",
                other
            ))),
        }
    }
}

impl BinarySerializer for Uri {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        self.value.serialize(context)
    }
}

impl BinaryDeserializer for Uri {
    fn deserialize(context: &mut DeserializationContext) -> desert_rust::Result<Self> {
        let value = String::deserialize(context)?;
        Ok(Uri { value })
    }
}

impl BinarySerializer for WitValue {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        self.nodes.serialize(context)
    }
}

impl BinaryDeserializer for WitValue {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let nodes = Vec::<WitNode>::deserialize(context)?;
        Ok(WitValue { nodes })
    }
}

lazy_static! {
    static ref WIT_NODE_METADATA: AdtMetadata = AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_RECORD_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_VARIANT_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_ENUM_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_FLAGS_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_TUPLE_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_LIST_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_OPTION_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_RESULT_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_PRIM_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_NODE_HANDLE_VALUE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_RECORD_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_VARIANT_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_ENUM_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_FLAGS_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_TUPLE_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_LIST_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_OPTION_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_RESULT_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_HANDLE_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref WIT_TYPE_NODE_PRIM_TYPE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref NAMED_WIT_TYPE_NODE_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
}

impl BinarySerializer for WitNode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let mut adt = AdtSerializer::<_, 1>::new_v0(&WIT_NODE_METADATA, context);
        match self {
            WitNode::RecordValue(field_indices) => {
                adt.write_constructor(0, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_RECORD_VALUE_METADATA, context);
                    inner.write_field("field_indices", field_indices)?;
                    inner.finish()
                })?;
            }
            WitNode::VariantValue((cons_idx, value_idx)) => {
                adt.write_constructor(1, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_VARIANT_VALUE_METADATA, context);
                    inner.write_field("cons_idx", cons_idx)?;
                    inner.write_field("value_idx", value_idx)?;
                    inner.finish()
                })?;
            }
            WitNode::EnumValue(value) => {
                adt.write_constructor(2, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_ENUM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::FlagsValue(values) => {
                adt.write_constructor(3, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_FLAGS_VALUE_METADATA, context);
                    inner.write_field("values", values)?;
                    inner.finish()
                })?;
            }
            WitNode::TupleValue(value_indices) => {
                adt.write_constructor(4, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_TUPLE_VALUE_METADATA, context);
                    inner.write_field("value_indices", value_indices)?;
                    inner.finish()
                })?;
            }
            WitNode::ListValue(value_indices) => {
                adt.write_constructor(5, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_LIST_VALUE_METADATA, context);
                    inner.write_field("value_indices", value_indices)?;
                    inner.finish()
                })?;
            }
            WitNode::OptionValue(opt_idx) => {
                adt.write_constructor(6, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_OPTION_VALUE_METADATA, context);
                    inner.write_field("opt_idx", opt_idx)?;
                    inner.finish()
                })?;
            }
            WitNode::ResultValue(res_idx) => {
                adt.write_constructor(7, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_RESULT_VALUE_METADATA, context);
                    inner.write_field("res_idx", res_idx)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimU8(value) => {
                adt.write_constructor(8, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimU16(value) => {
                adt.write_constructor(9, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimU32(value) => {
                adt.write_constructor(10, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimU64(value) => {
                adt.write_constructor(11, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimS8(value) => {
                adt.write_constructor(12, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimS16(value) => {
                adt.write_constructor(13, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimS32(value) => {
                adt.write_constructor(14, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimS64(value) => {
                adt.write_constructor(15, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimFloat32(value) => {
                adt.write_constructor(16, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimFloat64(value) => {
                adt.write_constructor(17, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimChar(value) => {
                adt.write_constructor(18, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimBool(value) => {
                adt.write_constructor(19, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::PrimString(value) => {
                adt.write_constructor(20, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_PRIM_VALUE_METADATA, context);
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
            WitNode::Handle((uri, value)) => {
                adt.write_constructor(21, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_NODE_HANDLE_VALUE_METADATA, context);
                    inner.write_field("uri", uri)?;
                    inner.write_field("value", value)?;
                    inner.finish()
                })?;
            }
        }
        adt.finish()
    }
}

impl BinarySerializer for WitTypeNode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let mut adt = AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_METADATA, context);
        match self {
            WitTypeNode::RecordType(field_types) => {
                adt.write_constructor(0, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_RECORD_TYPE_METADATA, context);
                    inner.write_field("field_types", field_types)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::VariantType(cons_types) => {
                adt.write_constructor(1, |context| {
                    let mut inner = AdtSerializer::<_, 1>::new_v0(
                        &WIT_TYPE_NODE_VARIANT_TYPE_METADATA,
                        context,
                    );
                    inner.write_field("cons_types", cons_types)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::EnumType(names) => {
                adt.write_constructor(2, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_ENUM_TYPE_METADATA, context);
                    inner.write_field("names", names)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::FlagsType(names) => {
                adt.write_constructor(3, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_FLAGS_TYPE_METADATA, context);
                    inner.write_field("names", names)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::TupleType(field_types) => {
                adt.write_constructor(4, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_TUPLE_TYPE_METADATA, context);
                    inner.write_field("field_types", field_types)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::ListType(elem_type) => {
                adt.write_constructor(5, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_LIST_TYPE_METADATA, context);
                    inner.write_field("elem_type", elem_type)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::OptionType(inner_type) => {
                adt.write_constructor(6, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_OPTION_TYPE_METADATA, context);
                    inner.write_field("inner_type", inner_type)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::ResultType((ok_type, err_type)) => {
                adt.write_constructor(7, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_RESULT_TYPE_METADATA, context);
                    inner.write_field("ok_type", ok_type)?;
                    inner.write_field("err_type", err_type)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::HandleType((id, mode)) => {
                adt.write_constructor(8, |context| {
                    let mut inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_HANDLE_TYPE_METADATA, context);
                    inner.write_field("id", id)?;
                    inner.write_field("mode", mode)?;
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimU8Type => {
                adt.write_constructor(9, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimU16Type => {
                adt.write_constructor(10, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimU32Type => {
                adt.write_constructor(11, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimU64Type => {
                adt.write_constructor(12, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimS8Type => {
                adt.write_constructor(13, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimS16Type => {
                adt.write_constructor(14, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimS32Type => {
                adt.write_constructor(15, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimS64Type => {
                adt.write_constructor(16, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimF32Type => {
                adt.write_constructor(17, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimF64Type => {
                adt.write_constructor(18, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimCharType => {
                adt.write_constructor(19, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimBoolType => {
                adt.write_constructor(20, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
            WitTypeNode::PrimStringType => {
                adt.write_constructor(21, |context| {
                    let inner =
                        AdtSerializer::<_, 1>::new_v0(&WIT_TYPE_NODE_PRIM_TYPE_METADATA, context);
                    inner.finish()
                })?;
            }
        }
        adt.finish()
    }
}

impl BinaryDeserializer for WitNode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        use desert_rust::BinaryInput;
        let stored_version = context.read_u8()?;
        let mut deserializer = if stored_version == 0 {
            desert_rust::adt::AdtDeserializer::<1>::new_v0(&WIT_NODE_METADATA, context)?
        } else {
            desert_rust::adt::AdtDeserializer::<1>::new(
                &WIT_NODE_METADATA,
                context,
                stored_version,
            )?
        };

        let constructor_idx = deserializer.read_constructor_idx()?;
        match constructor_idx {
            0 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_RECORD_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_RECORD_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let field_indices = inner_deserializer.read_field("field_indices", None)?;
                Ok(Self::RecordValue(field_indices))
            }
            1 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_VARIANT_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_VARIANT_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let cons_idx = inner_deserializer.read_field("cons_idx", None)?;
                let value_idx = inner_deserializer.read_field("value_idx", None)?;
                Ok(Self::VariantValue((cons_idx, value_idx)))
            }
            2 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_ENUM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_ENUM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::EnumValue(value))
            }
            3 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_FLAGS_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_FLAGS_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let values = inner_deserializer.read_field("values", None)?;
                Ok(Self::FlagsValue(values))
            }
            4 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_TUPLE_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_TUPLE_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value_indices = inner_deserializer.read_field("value_indices", None)?;
                Ok(Self::TupleValue(value_indices))
            }
            5 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_LIST_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_LIST_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value_indices = inner_deserializer.read_field("value_indices", None)?;
                Ok(Self::ListValue(value_indices))
            }
            6 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_OPTION_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_OPTION_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let opt_idx = inner_deserializer.read_field("opt_idx", None)?;
                Ok(Self::OptionValue(opt_idx))
            }
            7 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_RESULT_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_RESULT_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let res_idx = inner_deserializer.read_field("res_idx", None)?;
                Ok(Self::ResultValue(res_idx))
            }
            8 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimU8(value))
            }
            9 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimU16(value))
            }
            10 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimU32(value))
            }
            11 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimU64(value))
            }
            12 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimS8(value))
            }
            13 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimS16(value))
            }
            14 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimS32(value))
            }
            15 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimS64(value))
            }
            16 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimFloat32(value))
            }
            17 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimFloat64(value))
            }
            18 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimChar(value))
            }
            19 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimBool(value))
            }
            20 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_PRIM_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::PrimString(value))
            }
            21 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_NODE_HANDLE_VALUE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_NODE_HANDLE_VALUE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let uri = inner_deserializer.read_field("uri", None)?;
                let value = inner_deserializer.read_field("value", None)?;
                Ok(Self::Handle((uri, value)))
            }
            _ => Err(desert_rust::Error::InvalidConstructorId {
                type_name: "WitNode".to_string(),
                constructor_id: constructor_idx,
            }),
        }
    }
}

impl BinaryDeserializer for WitTypeNode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        use desert_rust::BinaryInput;
        let stored_version = context.read_u8()?;
        let mut deserializer = if stored_version == 0 {
            desert_rust::adt::AdtDeserializer::<1>::new_v0(&WIT_TYPE_NODE_METADATA, context)?
        } else {
            desert_rust::adt::AdtDeserializer::<1>::new(
                &WIT_TYPE_NODE_METADATA,
                context,
                stored_version,
            )?
        };

        let constructor_idx = deserializer.read_constructor_idx()?;
        match constructor_idx {
            0 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_RECORD_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_RECORD_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let field_types = inner_deserializer.read_field("field_types", None)?;
                Ok(Self::RecordType(field_types))
            }
            1 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_VARIANT_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_VARIANT_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let cons_types = inner_deserializer.read_field("cons_types", None)?;
                Ok(Self::VariantType(cons_types))
            }
            2 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_ENUM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_ENUM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let names = inner_deserializer.read_field("names", None)?;
                Ok(Self::EnumType(names))
            }
            3 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_FLAGS_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_FLAGS_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let names = inner_deserializer.read_field("names", None)?;
                Ok(Self::FlagsType(names))
            }
            4 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_TUPLE_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_TUPLE_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let field_types = inner_deserializer.read_field("field_types", None)?;
                Ok(Self::TupleType(field_types))
            }
            5 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_LIST_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_LIST_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let elem_type = inner_deserializer.read_field("elem_type", None)?;
                Ok(Self::ListType(elem_type))
            }
            6 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_OPTION_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_OPTION_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let inner_type = inner_deserializer.read_field("inner_type", None)?;
                Ok(Self::OptionType(inner_type))
            }
            7 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_RESULT_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_RESULT_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let ok_type = inner_deserializer.read_field("ok_type", None)?;
                let err_type = inner_deserializer.read_field("err_type", None)?;
                Ok(Self::ResultType((ok_type, err_type)))
            }
            8 => {
                let stored_version = context.read_u8()?;
                let mut inner_deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_HANDLE_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_HANDLE_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                let id = inner_deserializer.read_field("id", None)?;
                let mode = inner_deserializer.read_field("mode", None)?;
                Ok(Self::HandleType((id, mode)))
            }
            9 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimU8Type)
            }
            10 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimU16Type)
            }
            11 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimU32Type)
            }
            12 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimU64Type)
            }
            13 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimS8Type)
            }
            14 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimS16Type)
            }
            15 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimS32Type)
            }
            16 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimS64Type)
            }
            17 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimF32Type)
            }
            18 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimF64Type)
            }
            19 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimCharType)
            }
            20 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimBoolType)
            }
            21 => {
                let stored_version = context.read_u8()?;
                let _deserializer = if stored_version == 0 {
                    desert_rust::adt::AdtDeserializer::<1>::new_v0(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                    )?
                } else {
                    desert_rust::adt::AdtDeserializer::<1>::new(
                        &WIT_TYPE_NODE_PRIM_TYPE_METADATA,
                        context,
                        stored_version,
                    )?
                };
                Ok(Self::PrimStringType)
            }
            _ => Err(desert_rust::Error::InvalidConstructorId {
                type_name: "WitTypeNode".to_string(),
                constructor_id: constructor_idx,
            }),
        }
    }
}

impl BinarySerializer for NamedWitTypeNode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let mut adt = AdtSerializer::<_, 1>::new_v0(&NAMED_WIT_TYPE_NODE_METADATA, context);
        adt.write_field("name", &self.name)?;
        adt.write_field("owner", &self.owner)?;
        adt.write_field("type_", &self.type_)?;
        adt.finish()
    }
}

impl BinaryDeserializer for NamedWitTypeNode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        use desert_rust::BinaryInput;
        let stored_version = context.read_u8()?;
        let mut deserializer = if stored_version == 0 {
            desert_rust::adt::AdtDeserializer::<1>::new_v0(&NAMED_WIT_TYPE_NODE_METADATA, context)?
        } else {
            desert_rust::adt::AdtDeserializer::<1>::new(
                &NAMED_WIT_TYPE_NODE_METADATA,
                context,
                stored_version,
            )?
        };

        let name = deserializer.read_field("name", None)?;
        let owner = deserializer.read_field("owner", None)?;
        let type_ = deserializer.read_field("type_", None)?;
        Ok(Self { name, owner, type_ })
    }
}

impl BinarySerializer for WitType {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        self.nodes.serialize(context)
    }
}

impl BinaryDeserializer for WitType {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let nodes = Vec::<NamedWitTypeNode>::deserialize(context)?;
        Ok(WitType { nodes })
    }
}

impl BinarySerializer for ResourceMode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match self {
            ResourceMode::Borrowed => 0u8.serialize(context),
            ResourceMode::Owned => 1u8.serialize(context),
        }
    }
}

impl BinaryDeserializer for ResourceMode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag: u8 = u8::deserialize(context)?;
        match tag {
            0 => Ok(ResourceMode::Borrowed),
            1 => Ok(ResourceMode::Owned),
            _ => Err(desert_rust::Error::InvalidConstructorId {
                type_name: "ResourceMode".to_string(),
                constructor_id: tag as u32,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::{Value, WitValue};
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb_sized;

    const CASES: u32 = 10000;
    const SIZE: usize = 4096;

    proptest! {

        #![proptest_config(ProptestConfig {
            cases: CASES, .. ProptestConfig::default()
        })]
        #[test]
        fn round_trip_wit_value(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: WitValue = value.clone().into();
            let encoded = desert_rust::serialize_to_byte_vec(&wit_value).unwrap();
            let decoded: WitValue = desert_rust::deserialize(&encoded).unwrap();
            let round_trip_value: Value = decoded.into();
            prop_assert_eq!(value, round_trip_value);
        }

        #[test]
        fn round_trip_value(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let encoded = desert_rust::serialize_to_byte_vec(&value).unwrap();
            let decoded: Value = desert_rust::deserialize(&encoded).unwrap();
            prop_assert_eq!(value, decoded);
        }
    }
}
