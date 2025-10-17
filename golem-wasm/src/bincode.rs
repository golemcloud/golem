use crate::golem_rpc_0_2_x::types::{NamedWitTypeNode, ResourceId};
use crate::{NodeIndex, ResourceMode, Uri, WitNode, WitType, WitTypeNode, WitValue};
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{AllowedEnumVariants, DecodeError, EncodeError};
use bincode::*;

impl Encode for Uri {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.value.encode(encoder)
    }
}

impl<Context> Decode<Context> for Uri {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let value = String::decode(decoder)?;
        Ok(Uri { value })
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for Uri {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let value = String::borrow_decode(decoder)?;
        Ok(Uri { value })
    }
}

impl Encode for WitValue {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.nodes.encode(encoder)
    }
}

impl<Context> Decode<Context> for WitValue {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let nodes = Vec::<WitNode>::decode(decoder)?;
        Ok(WitValue { nodes })
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for WitValue {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let nodes = Vec::<WitNode>::borrow_decode(decoder)?;
        Ok(WitValue { nodes })
    }
}

impl Encode for WitNode {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            WitNode::RecordValue(field_indices) => {
                0u8.encode(encoder)?;
                field_indices.encode(encoder)
            }
            WitNode::VariantValue((cons_idx, value_idx)) => {
                1u8.encode(encoder)?;
                cons_idx.encode(encoder)?;
                value_idx.encode(encoder)
            }
            WitNode::EnumValue(value) => {
                2u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::FlagsValue(values) => {
                3u8.encode(encoder)?;
                values.encode(encoder)
            }
            WitNode::TupleValue(value_indices) => {
                4u8.encode(encoder)?;
                value_indices.encode(encoder)
            }
            WitNode::ListValue(value_indices) => {
                5u8.encode(encoder)?;
                value_indices.encode(encoder)
            }
            WitNode::OptionValue(opt_idx) => {
                6u8.encode(encoder)?;
                opt_idx.encode(encoder)
            }
            WitNode::ResultValue(res_idx) => {
                7u8.encode(encoder)?;
                res_idx.encode(encoder)
            }
            WitNode::PrimU8(value) => {
                8u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimU16(value) => {
                9u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimU32(value) => {
                10u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimU64(value) => {
                11u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimS8(value) => {
                12u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimS16(value) => {
                13u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimS32(value) => {
                14u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimS64(value) => {
                15u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimFloat32(value) => {
                16u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimFloat64(value) => {
                17u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimChar(value) => {
                18u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimBool(value) => {
                19u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::PrimString(value) => {
                20u8.encode(encoder)?;
                value.encode(encoder)
            }
            WitNode::Handle((uri, value)) => {
                21u8.encode(encoder)?;
                uri.value.encode(encoder)?;
                value.encode(encoder)
            }
        }
    }
}

impl<Context> Decode<Context> for WitNode {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let tag: u8 = Decode::decode(decoder)?;
        match tag {
            0u8 => {
                let field_indices = Vec::<i32>::decode(decoder)?;
                Ok(WitNode::RecordValue(field_indices))
            }
            1u8 => {
                let cons_idx = u32::decode(decoder)?;
                let value_idx = Option::<i32>::decode(decoder)?;
                Ok(WitNode::VariantValue((cons_idx, value_idx)))
            }
            2u8 => {
                let value = u32::decode(decoder)?;
                Ok(WitNode::EnumValue(value))
            }
            3u8 => {
                let values = Vec::<bool>::decode(decoder)?;
                Ok(WitNode::FlagsValue(values))
            }
            4u8 => {
                let value_indices = Vec::<i32>::decode(decoder)?;
                Ok(WitNode::TupleValue(value_indices))
            }
            5u8 => {
                let value_indices = Vec::<i32>::decode(decoder)?;
                Ok(WitNode::ListValue(value_indices))
            }
            6u8 => {
                let opt_idx = Option::<i32>::decode(decoder)?;
                Ok(WitNode::OptionValue(opt_idx))
            }
            7u8 => {
                let res_idx = Result::<Option<i32>, Option<i32>>::decode(decoder)?;
                Ok(WitNode::ResultValue(res_idx))
            }
            8u8 => {
                let value = u8::decode(decoder)?;
                Ok(WitNode::PrimU8(value))
            }
            9u8 => {
                let value = u16::decode(decoder)?;
                Ok(WitNode::PrimU16(value))
            }
            10u8 => {
                let value = u32::decode(decoder)?;
                Ok(WitNode::PrimU32(value))
            }
            11u8 => {
                let value = u64::decode(decoder)?;
                Ok(WitNode::PrimU64(value))
            }
            12u8 => {
                let value = i8::decode(decoder)?;
                Ok(WitNode::PrimS8(value))
            }
            13u8 => {
                let value = i16::decode(decoder)?;
                Ok(WitNode::PrimS16(value))
            }
            14u8 => {
                let value = i32::decode(decoder)?;
                Ok(WitNode::PrimS32(value))
            }
            15u8 => {
                let value = i64::decode(decoder)?;
                Ok(WitNode::PrimS64(value))
            }
            16u8 => {
                let value = f32::decode(decoder)?;
                Ok(WitNode::PrimFloat32(value))
            }
            17u8 => {
                let value = f64::decode(decoder)?;
                Ok(WitNode::PrimFloat64(value))
            }
            18u8 => {
                let value = char::decode(decoder)?;
                Ok(WitNode::PrimChar(value))
            }
            19u8 => {
                let value = bool::decode(decoder)?;
                Ok(WitNode::PrimBool(value))
            }
            20u8 => {
                let value = String::decode(decoder)?;
                Ok(WitNode::PrimString(value))
            }
            21u8 => {
                let uri = String::decode(decoder)?;
                let value = u64::decode(decoder)?;
                Ok(WitNode::Handle((Uri { value: uri }, value)))
            }
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "WitNode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 21 },
            }),
        }
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for WitNode {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let tag: u8 = BorrowDecode::borrow_decode(decoder)?;
        match tag {
            0u8 => {
                let field_indices = Vec::<i32>::borrow_decode(decoder)?;
                Ok(WitNode::RecordValue(field_indices))
            }
            1u8 => {
                let cons_idx = u32::borrow_decode(decoder)?;
                let value_idx = Option::<i32>::borrow_decode(decoder)?;
                Ok(WitNode::VariantValue((cons_idx, value_idx)))
            }
            2u8 => {
                let value = u32::borrow_decode(decoder)?;
                Ok(WitNode::EnumValue(value))
            }
            3u8 => {
                let values = Vec::<bool>::borrow_decode(decoder)?;
                Ok(WitNode::FlagsValue(values))
            }
            4u8 => {
                let value_indices = Vec::<i32>::borrow_decode(decoder)?;
                Ok(WitNode::TupleValue(value_indices))
            }
            5u8 => {
                let value_indices = Vec::<i32>::borrow_decode(decoder)?;
                Ok(WitNode::ListValue(value_indices))
            }
            6u8 => {
                let opt_idx = Option::<i32>::borrow_decode(decoder)?;
                Ok(WitNode::OptionValue(opt_idx))
            }
            7u8 => {
                let res_idx = Result::<Option<i32>, Option<i32>>::borrow_decode(decoder)?;
                Ok(WitNode::ResultValue(res_idx))
            }
            8u8 => {
                let value = u8::borrow_decode(decoder)?;
                Ok(WitNode::PrimU8(value))
            }
            9u8 => {
                let value = u16::borrow_decode(decoder)?;
                Ok(WitNode::PrimU16(value))
            }
            10u8 => {
                let value = u32::borrow_decode(decoder)?;
                Ok(WitNode::PrimU32(value))
            }
            11u8 => {
                let value = u64::borrow_decode(decoder)?;
                Ok(WitNode::PrimU64(value))
            }
            12u8 => {
                let value = i8::borrow_decode(decoder)?;
                Ok(WitNode::PrimS8(value))
            }
            13u8 => {
                let value = i16::borrow_decode(decoder)?;
                Ok(WitNode::PrimS16(value))
            }
            14u8 => {
                let value = i32::borrow_decode(decoder)?;
                Ok(WitNode::PrimS32(value))
            }
            15u8 => {
                let value = i64::borrow_decode(decoder)?;
                Ok(WitNode::PrimS64(value))
            }
            16u8 => {
                let value = f32::borrow_decode(decoder)?;
                Ok(WitNode::PrimFloat32(value))
            }
            17u8 => {
                let value = f64::borrow_decode(decoder)?;
                Ok(WitNode::PrimFloat64(value))
            }
            18u8 => {
                let value = char::borrow_decode(decoder)?;
                Ok(WitNode::PrimChar(value))
            }
            19u8 => {
                let value = bool::borrow_decode(decoder)?;
                Ok(WitNode::PrimBool(value))
            }
            20u8 => {
                let value = String::borrow_decode(decoder)?;
                Ok(WitNode::PrimString(value))
            }
            21u8 => {
                let uri = String::borrow_decode(decoder)?;
                let value = u64::borrow_decode(decoder)?;
                Ok(WitNode::Handle((Uri { value: uri }, value)))
            }
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "WitNode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 21 },
            }),
        }
    }
}

impl Encode for WitType {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.nodes.encode(encoder)
    }
}

impl<Context> Decode<Context> for WitType {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let nodes = Vec::<NamedWitTypeNode>::decode(decoder)?;
        Ok(WitType { nodes })
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for WitType {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let nodes = Vec::<NamedWitTypeNode>::borrow_decode(decoder)?;
        Ok(WitType { nodes })
    }
}

impl Encode for ResourceMode {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            ResourceMode::Borrowed => 0u8.encode(encoder),
            ResourceMode::Owned => 1u8.encode(encoder),
        }
    }
}

impl<Context> Decode<Context> for ResourceMode {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let tag: u8 = Decode::decode(decoder)?;
        match tag {
            0u8 => Ok(ResourceMode::Borrowed),
            1u8 => Ok(ResourceMode::Owned),
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "ResourceMode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 1 },
            }),
        }
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for ResourceMode {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let tag: u8 = BorrowDecode::borrow_decode(decoder)?;
        match tag {
            0u8 => Ok(ResourceMode::Borrowed),
            1u8 => Ok(ResourceMode::Owned),
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "ResourceMode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 1 },
            }),
        }
    }
}

impl Encode for NamedWitTypeNode {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.name.encode(encoder)?;
        self.owner.encode(encoder)?;
        self.type_.encode(encoder)
    }
}

impl<Context> Decode<Context> for NamedWitTypeNode {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let name = Option::<String>::decode(decoder)?;
        let owner = Option::<String>::decode(decoder)?;
        let type_ = WitTypeNode::decode(decoder)?;
        Ok(NamedWitTypeNode { name, owner, type_ })
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for NamedWitTypeNode {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let name = Option::<String>::borrow_decode(decoder)?;
        let owner = Option::<String>::borrow_decode(decoder)?;
        let type_ = WitTypeNode::borrow_decode(decoder)?;
        Ok(NamedWitTypeNode { name, owner, type_ })
    }
}

impl Encode for WitTypeNode {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            WitTypeNode::RecordType(field_types) => {
                0u8.encode(encoder)?;
                field_types.encode(encoder)
            }
            WitTypeNode::VariantType(cons_types) => {
                1u8.encode(encoder)?;
                cons_types.encode(encoder)
            }
            WitTypeNode::EnumType(names) => {
                2u8.encode(encoder)?;
                names.encode(encoder)
            }
            WitTypeNode::FlagsType(names) => {
                3u8.encode(encoder)?;
                names.encode(encoder)
            }
            WitTypeNode::TupleType(field_types) => {
                4u8.encode(encoder)?;
                field_types.encode(encoder)
            }
            WitTypeNode::ListType(elem_type) => {
                5u8.encode(encoder)?;
                elem_type.encode(encoder)
            }
            WitTypeNode::OptionType(inner_type) => {
                6u8.encode(encoder)?;
                inner_type.encode(encoder)
            }
            WitTypeNode::ResultType((ok_type, err_type)) => {
                7u8.encode(encoder)?;
                ok_type.encode(encoder)?;
                err_type.encode(encoder)
            }
            WitTypeNode::HandleType((id, mode)) => {
                8u8.encode(encoder)?;
                id.encode(encoder)?;
                mode.encode(encoder)
            }
            WitTypeNode::PrimU8Type => 9u8.encode(encoder),
            WitTypeNode::PrimU16Type => 10u8.encode(encoder),
            WitTypeNode::PrimU32Type => 11u8.encode(encoder),
            WitTypeNode::PrimU64Type => 12u8.encode(encoder),
            WitTypeNode::PrimS8Type => 13u8.encode(encoder),
            WitTypeNode::PrimS16Type => 14u8.encode(encoder),
            WitTypeNode::PrimS32Type => 15u8.encode(encoder),
            WitTypeNode::PrimS64Type => 16u8.encode(encoder),
            WitTypeNode::PrimF32Type => 17u8.encode(encoder),
            WitTypeNode::PrimF64Type => 18u8.encode(encoder),
            WitTypeNode::PrimCharType => 19u8.encode(encoder),
            WitTypeNode::PrimBoolType => 20u8.encode(encoder),
            WitTypeNode::PrimStringType => 21u8.encode(encoder),
        }
    }
}

impl<Context> Decode<Context> for WitTypeNode {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let tag: u8 = Decode::decode(decoder)?;
        match tag {
            0u8 => {
                let field_types = Vec::<(String, NodeIndex)>::decode(decoder)?;
                Ok(WitTypeNode::RecordType(field_types))
            }
            1u8 => {
                let cons_types = Vec::<(String, Option<NodeIndex>)>::decode(decoder)?;
                Ok(WitTypeNode::VariantType(cons_types))
            }
            2u8 => {
                let names = Vec::<String>::decode(decoder)?;
                Ok(WitTypeNode::EnumType(names))
            }
            3u8 => {
                let names = Vec::<String>::decode(decoder)?;
                Ok(WitTypeNode::FlagsType(names))
            }
            4u8 => {
                let field_types = Vec::<NodeIndex>::decode(decoder)?;
                Ok(WitTypeNode::TupleType(field_types))
            }
            5u8 => {
                let elem_type = NodeIndex::decode(decoder)?;
                Ok(WitTypeNode::ListType(elem_type))
            }
            6u8 => {
                let inner_type = NodeIndex::decode(decoder)?;
                Ok(WitTypeNode::OptionType(inner_type))
            }
            7u8 => {
                let ok_type = Option::<NodeIndex>::decode(decoder)?;
                let err_type = Option::<NodeIndex>::decode(decoder)?;
                Ok(WitTypeNode::ResultType((ok_type, err_type)))
            }
            8u8 => {
                let resource_id = ResourceId::decode(decoder)?;
                let resource_mode = ResourceMode::decode(decoder)?;
                Ok(WitTypeNode::HandleType((resource_id, resource_mode)))
            }
            9u8 => Ok(WitTypeNode::PrimU8Type),
            10u8 => Ok(WitTypeNode::PrimU16Type),
            11u8 => Ok(WitTypeNode::PrimU32Type),
            12u8 => Ok(WitTypeNode::PrimU64Type),
            13u8 => Ok(WitTypeNode::PrimS8Type),
            14u8 => Ok(WitTypeNode::PrimS16Type),
            15u8 => Ok(WitTypeNode::PrimS32Type),
            16u8 => Ok(WitTypeNode::PrimS64Type),
            17u8 => Ok(WitTypeNode::PrimF32Type),
            18u8 => Ok(WitTypeNode::PrimF64Type),
            19u8 => Ok(WitTypeNode::PrimCharType),
            20u8 => Ok(WitTypeNode::PrimBoolType),
            21u8 => Ok(WitTypeNode::PrimStringType),
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "WitTypeNode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 9 },
            }),
        }
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for WitTypeNode {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let tag: u8 = BorrowDecode::borrow_decode(decoder)?;
        match tag {
            0u8 => {
                let field_types = Vec::<(String, NodeIndex)>::borrow_decode(decoder)?;
                Ok(WitTypeNode::RecordType(field_types))
            }
            1u8 => {
                let cons_types = Vec::<(String, Option<NodeIndex>)>::borrow_decode(decoder)?;
                Ok(WitTypeNode::VariantType(cons_types))
            }
            2u8 => {
                let names = Vec::<String>::borrow_decode(decoder)?;
                Ok(WitTypeNode::EnumType(names))
            }
            3u8 => {
                let names = Vec::<String>::borrow_decode(decoder)?;
                Ok(WitTypeNode::FlagsType(names))
            }
            4u8 => {
                let field_types = Vec::<NodeIndex>::borrow_decode(decoder)?;
                Ok(WitTypeNode::TupleType(field_types))
            }
            5u8 => {
                let elem_type = NodeIndex::borrow_decode(decoder)?;
                Ok(WitTypeNode::ListType(elem_type))
            }
            6u8 => {
                let inner_type = NodeIndex::borrow_decode(decoder)?;
                Ok(WitTypeNode::OptionType(inner_type))
            }
            7u8 => {
                let ok_type = Option::<NodeIndex>::borrow_decode(decoder)?;
                let err_type = Option::<NodeIndex>::borrow_decode(decoder)?;
                Ok(WitTypeNode::ResultType((ok_type, err_type)))
            }
            8u8 => {
                let resource_id = ResourceId::borrow_decode(decoder)?;
                let resource_mode = ResourceMode::borrow_decode(decoder)?;
                Ok(WitTypeNode::HandleType((resource_id, resource_mode)))
            }
            9u8 => Ok(WitTypeNode::PrimU8Type),
            10u8 => Ok(WitTypeNode::PrimU16Type),
            11u8 => Ok(WitTypeNode::PrimU32Type),
            12u8 => Ok(WitTypeNode::PrimU64Type),
            13u8 => Ok(WitTypeNode::PrimS8Type),
            14u8 => Ok(WitTypeNode::PrimS16Type),
            15u8 => Ok(WitTypeNode::PrimS32Type),
            16u8 => Ok(WitTypeNode::PrimS64Type),
            17u8 => Ok(WitTypeNode::PrimF32Type),
            18u8 => Ok(WitTypeNode::PrimF64Type),
            19u8 => Ok(WitTypeNode::PrimCharType),
            20u8 => Ok(WitTypeNode::PrimBoolType),
            21u8 => Ok(WitTypeNode::PrimStringType),
            _ => Err(DecodeError::UnexpectedVariant {
                found: tag as u32,
                type_name: "WitTypeNode",
                allowed: &AllowedEnumVariants::Range { min: 0, max: 9 },
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
            let encoded = bincode::encode_to_vec(wit_value, bincode::config::standard()).unwrap();
            let (decoded, _): (WitValue, usize) = bincode::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
            let round_trip_value: Value = decoded.into();
            prop_assert_eq!(value, round_trip_value);
        }

        #[test]
        fn round_trip_value(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let encoded = bincode::encode_to_vec(value.clone(), bincode::config::standard()).unwrap();
            let (decoded, _): (Value, usize) = bincode::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
            prop_assert_eq!(value, decoded);
        }
    }
}
