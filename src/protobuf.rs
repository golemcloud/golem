use crate::Value;
include!(concat!(env!("OUT_DIR"), "/wasm.rpc.rs"));

// Conversion from WIT WitValue to Protobuf WitValue

impl From<super::WitValue> for WitValue {
    fn from(value: super::WitValue) -> Self {
        WitValue {
            nodes: value.nodes.into_iter().map(|node| node.into()).collect(),
        }
    }
}

impl From<super::WitNode> for WitNode {
    fn from(value: super::WitNode) -> Self {
        match value {
            super::WitNode::RecordValue(fields) =>
                WitNode {
                    value: Some(wit_node::Value::Record(WitRecordNode {
                        fields: fields.into_iter().map(|(name, index)|
                            NameTypeIndex {
                                name,
                                index,
                            }
                        ).collect(),
                    }))
                },
            super::WitNode::VariantValue((case_name, type_idx)) =>
                WitNode {
                    value: Some(wit_node::Value::Variant(WitVariantNode {
                        variant: Some(NameTypeIndex {
                            name: case_name,
                            index: type_idx,
                        })
                    }))
                },
            super::WitNode::EnumValue(value) =>
                WitNode {
                    value: Some(wit_node::Value::Enum(WitEnumNode {
                        value
                    }))
                },
            super::WitNode::FlagsValue(flags) =>
                WitNode {
                    value: Some(wit_node::Value::Flags(WitFlagsNode { flags }))
                },
            super::WitNode::TupleValue(values) =>
                WitNode {
                    value: Some(wit_node::Value::Tuple(WitTupleNode {
                        values
                    }))
                },
            super::WitNode::ListValue(values) =>
                WitNode {
                    value: Some(wit_node::Value::List(WitListNode { values }))
                },
            super::WitNode::OptionValue(value) =>
                WitNode {
                    value: Some(wit_node::Value::Option(WitOptionNode { value }))
                },
            super::WitNode::ResultValue(type_idx) =>
                WitNode {
                    value: Some(wit_node::Value::Result(
                        WitResultNode {
                            ok: type_idx.ok(),
                            err: type_idx.err(),
                        }
                    ))
                },
            super::WitNode::PrimU8(value) =>
                WitNode {
                    value: Some(wit_node::Value::U8(WitPrimU8Node { value: value as u32 })),
                },
            super::WitNode::PrimU16(value) =>
                WitNode {
                    value: Some(wit_node::Value::U16(WitPrimU16Node { value: value as u32 })),
                },
            super::WitNode::PrimU32(value) =>
                WitNode {
                    value: Some(wit_node::Value::U32(WitPrimU32Node { value })),
                },
            super::WitNode::PrimU64(value) =>
                WitNode {
                    value: Some(wit_node::Value::U64(WitPrimU64Node { value })),
                },
            super::WitNode::PrimS8(value) =>
                WitNode {
                    value: Some(wit_node::Value::I8(WitPrimI8Node { value: value as i32 })),
                },
            super::WitNode::PrimS16(value) =>
                WitNode {
                    value: Some(wit_node::Value::I16(WitPrimI16Node { value: value as i32 })),
                },
            super::WitNode::PrimS32(value) =>
                WitNode {
                    value: Some(wit_node::Value::I32(WitPrimI32Node { value })),
                },
            super::WitNode::PrimS64(value) =>
                WitNode {
                    value: Some(wit_node::Value::I64(WitPrimI64Node { value })),
                },
            super::WitNode::PrimFloat32(value) =>
                WitNode {
                    value: Some(wit_node::Value::F32(WitPrimF32Node { value })),
                },
            super::WitNode::PrimFloat64(value) =>
                WitNode {
                    value: Some(wit_node::Value::F64(WitPrimF64Node { value })),
                },
            super::WitNode::PrimChar(value) =>
                WitNode {
                    value: Some(wit_node::Value::Char(WitPrimCharNode { value: value as u32 })),
                },
            super::WitNode::PrimBool(value) =>
                WitNode {
                    value: Some(wit_node::Value::Bool(WitPrimBoolNode { value })),
                },
            super::WitNode::PrimString(value) =>
                WitNode {
                    value: Some(wit_node::Value::String(WitPrimStringNode { value })),
                }
        }
    }
}

// Conversion from Protobuf WitValue to WIT WitValue
impl TryFrom<WitValue> for super::WitValue {
    type Error = String;

    fn try_from(value: WitValue) -> Result<Self, Self::Error> {
        Ok(super::WitValue {
            nodes: value.nodes.into_iter().map(|node| node.try_into()).collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl TryFrom<WitNode> for super::WitNode {
    type Error = String;

    fn try_from(value: WitNode) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Protobuf WitNode has no value".to_string()),
            Some(wit_node::Value::Record(WitRecordNode { fields })) =>
                Ok(super::WitNode::RecordValue(fields.into_iter().map(|NameTypeIndex { name, index }| (name, index)).collect())),
            Some(wit_node::Value::Variant(WitVariantNode { variant })) => {
                let variant = variant.ok_or("Protobuf WitVariantNode has no variant")?;
                Ok(super::WitNode::VariantValue((variant.name, variant.index)))
            }
            Some(wit_node::Value::Enum(WitEnumNode { value })) =>
                Ok(super::WitNode::EnumValue(value)),
            Some(wit_node::Value::Flags(WitFlagsNode { flags })) =>
                Ok(super::WitNode::FlagsValue(flags)),
            Some(wit_node::Value::Tuple(WitTupleNode { values })) =>
                Ok(super::WitNode::TupleValue(values)),
            Some(wit_node::Value::List(WitListNode { values })) =>
                Ok(super::WitNode::ListValue(values)),
            Some(wit_node::Value::Option(WitOptionNode { value })) =>
                Ok(super::WitNode::OptionValue(value)),
            Some(wit_node::Value::Result(WitResultNode { ok, err })) =>
                match (ok, err) {
                    (Some(_), Some(_)) => Err("Protobuf WitResultNode has both ok and err".to_string()),
                    (Some(ok), None) => Ok(super::WitNode::ResultValue(Ok(ok))),
                    (None, Some(err)) => Ok(super::WitNode::ResultValue(Err(err))),
                    (None, None) => Err("Protobuf WitResultNode has neither ok nor err".to_string()),
                }
            Some(wit_node::Value::U8(WitPrimU8Node { value })) =>
                Ok(super::WitNode::PrimU8(value as u8)),
            Some(wit_node::Value::U16(WitPrimU16Node { value })) =>
                Ok(super::WitNode::PrimU16(value as u16)),
            Some(wit_node::Value::U32(WitPrimU32Node { value })) =>
                Ok(super::WitNode::PrimU32(value)),
            Some(wit_node::Value::U64(WitPrimU64Node { value })) =>
                Ok(super::WitNode::PrimU64(value)),
            Some(wit_node::Value::I8(WitPrimI8Node { value })) =>
                Ok(super::WitNode::PrimS8(value as i8)),
            Some(wit_node::Value::I16(WitPrimI16Node { value })) =>
                Ok(super::WitNode::PrimS16(value as i16)),
            Some(wit_node::Value::I32(WitPrimI32Node { value })) =>
                Ok(super::WitNode::PrimS32(value)),
            Some(wit_node::Value::I64(WitPrimI64Node { value })) =>
                Ok(super::WitNode::PrimS64(value)),
            Some(wit_node::Value::F32(WitPrimF32Node { value })) =>
                Ok(super::WitNode::PrimFloat32(value)),
            Some(wit_node::Value::F64(WitPrimF64Node { value })) =>
                Ok(super::WitNode::PrimFloat64(value)),
            Some(wit_node::Value::Char(WitPrimCharNode { value })) =>
                Ok(super::WitNode::PrimChar(char::from_u32(value).ok_or("Protobuf WitPrimCharNode has invalid value".to_string())?)),
            Some(wit_node::Value::Bool(WitPrimBoolNode { value })) =>
                Ok(super::WitNode::PrimBool(value)),
            Some(wit_node::Value::String(WitPrimStringNode { value })) =>
                Ok(super::WitNode::PrimString(value)),
        }
    }
}

// Conversion from WitValue to protobuf Val
impl From<WitValue> for Val {
    fn from(value: WitValue) -> Self {
        let value: Value = value.into();
        value.into()
    }
}

impl From<Value> for Val {
    fn from(value: Value) -> Self {
        match value {
            Value::Bool(value) => Val { val: Some(val::Val::Bool(value)) },
            Value::U8(value) => Val { val: Some(val::Val::U8(value as i32)) },
            Value::U16(value) => Val { val: Some(val::Val::U16(value as i32)) },
            Value::U32(value) => Val { val: Some(val::Val::U32(value as i64)) },
            Value::U64(value) => Val { val: Some(val::Val::U64(value as i64)) },
            Value::I8(value) => Val { val: Some(val::Val::S8(value as i32)) },
            Value::I16(value) => Val { val: Some(val::Val::S16(value as i32)) },
            Value::I32(value) => Val { val: Some(val::Val::S32(value)) },
            Value::I64(value) => Val { val: Some(val::Val::S64(value)) },
            Value::F32(value) => Val { val: Some(val::Val::F32(value)) },
            Value::F64(value) => Val { val: Some(val::Val::F64(value)) },
            Value::Char(value) => Val { val: Some(val::Val::Char(value as i32)) },
            Value::String(value) => Val { val: Some(val::Val::String(value)) },
            Value::List(items) => Val {
                val: Some(val::Val::List(
                    ValList { values: items.into_iter().map(|item| item.into()).collect() }
                ))
            },
            Value::Tuple(items) => Val {
                val: Some(val::Val::Tuple(
                    ValTuple { values: items.into_iter().map(|item| item.into()).collect() }
                ))
            },
            Value::Record(fields) => Val {
                val: Some(val::Val::Record(
                    ValRecord { values: fields.into_iter().map(|(_, value)| value.into()).collect() }
                ))
            },
            Value::Variant(_, value) => Val {
                val: Some(val::Val::Variant(
                    Box::new(ValVariant {
                        discriminant: -1, // TODO
                        value: Some(Box::new(value.into())),
                    })
                ))
            },
            Value::Enum(value) => Val { val: Some(val::Val::Enum(ValEnum {
                discriminant: -1, // TODO
            })) },
            Value::Flags(_) => {}
            Value::Option(_) => {}
            Value::Result(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest_arbitrary_interop::{arb, arb_sized};
    use crate::Value;
    use super::WitValue;

    const CASES: u32 = 10000;
    const SIZE: usize = 4096;

    proptest! {

        #![proptest_config(ProptestConfig {
            cases: CASES, .. ProptestConfig::default()
        })]
        #[test]
        fn round_trip(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(&v))) {
            let wit_value: crate::WitValue = value.clone().into();
            let protobuf_wit_value: WitValue = wit_value.into();
            let round_trip_wit_value: crate::WitValue = protobuf_wit_value.try_into().unwrap();
            let round_trip_value: Value = round_trip_wit_value.into();
            prop_assert_eq!(value, round_trip_value);
        }
    }
}
