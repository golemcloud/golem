#[macro_export]
macro_rules! newtype_uuid {
    ($name:ident, $proto_type:path) => {
        #[derive(
            Clone, Debug, PartialOrd, Ord, FromStr, Eq, Hash, PartialEq, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new_v4() -> $name {
                Self(Uuid::new_v4())
            }
        }

        impl Encode for $name {
            fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
                encoder.writer().write(self.0.as_bytes())
            }
        }

        impl Decode for $name {
            fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
                let mut bytes = [0u8; 16];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(Uuid::from_bytes(bytes)))
            }
        }

        impl<'de> BorrowDecode<'de> for $name {
            fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
                let mut bytes = [0u8; 16];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(Uuid::from_bytes(bytes)))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = String;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                let uuid =
                    Uuid::parse_str(value).map_err(|err| format!("Invalid plan ID: {err}"))?;
                Ok(Self(uuid))
            }
        }

        impl TryFrom<$proto_type> for $name {
            type Error = String;

            fn try_from(value: $proto_type) -> Result<Self, Self::Error> {
                Ok(Self(
                    value
                        .value
                        .ok_or(format!("Missing value in {}", stringify!($name)))?
                        .into(),
                ))
            }
        }

        impl From<$name> for $proto_type {
            fn from(value: $name) -> Self {
                $proto_type {
                    value: Some(value.0.into()),
                }
            }
        }

        impl Type for $name {
            const IS_REQUIRED: bool = true;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> Cow<'static, str> {
                Cow::from(format!("string({})", stringify!($name)))
            }

            fn schema_ref() -> MetaSchemaRef {
                MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "uuid")))
            }

            fn as_raw_value(&self) -> Option<&Self::RawValueType> {
                Some(self)
            }

            fn raw_element_iter<'a>(
                &'a self,
            ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
                Box::new(self.as_raw_value().into_iter())
            }
        }

        impl ParseFromParameter for $name {
            fn parse_from_parameter(value: &str) -> ParseResult<Self> {
                Ok(Self(value.try_into()?))
            }
        }

        impl ParseFromJSON for $name {
            fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
                match value {
                    Some(Value::String(s)) => Ok(Self(Uuid::from_str(&s)?)),
                    _ => Err(poem_openapi::types::ParseError::<$name>::custom(format!(
                        "Unexpected representation of {}",
                        stringify!($name)
                    ))),
                }
            }
        }

        impl ToJSON for $name {
            fn to_json(&self) -> Option<Value> {
                Some(Value::String(self.0.to_string()))
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", &self.0)
            }
        }
    };
}
