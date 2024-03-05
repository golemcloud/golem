use crate::{Uri, WitNode, WitValue};
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{AllowedEnumVariants, DecodeError, EncodeError};
use bincode::*;

impl Encode for WitValue {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.nodes.encode(encoder)
    }
}

impl Decode for WitValue {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let nodes = Vec::<WitNode>::decode(decoder)?;
        Ok(WitValue { nodes })
    }
}

impl<'de> BorrowDecode<'de> for WitValue {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
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

impl Decode for WitNode {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
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

impl<'de> BorrowDecode<'de> for WitNode {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
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

#[cfg(test)]
mod tests {
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
        fn round_trip(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: WitValue = value.clone().into();
            let encoded = bincode::encode_to_vec(wit_value, bincode::config::standard()).unwrap();
            let (decoded, _): (WitValue, usize) = bincode::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
            let round_trip_value: Value = decoded.into();
            prop_assert_eq!(value, round_trip_value);
        }
    }
}
