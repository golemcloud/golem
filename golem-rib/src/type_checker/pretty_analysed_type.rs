use golem_wasm_ast::analysis::AnalysedType;
use std::fmt::{Display, Formatter};
use std::ops::Deref;

pub struct PrettyAnalysedType(pub AnalysedType);

impl Display for PrettyAnalysedType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            AnalysedType::Record(fields) => {
                write!(f, "record {{")?;
                for field in fields.fields.iter() {
                    write!(
                        f,
                        "{}: {}, ",
                        field.name,
                        PrettyAnalysedType(field.clone().typ)
                    )?;
                }
                write!(f, "}}")
            }

            AnalysedType::S32(_) => write!(f, "s32"),
            AnalysedType::U64(_) => write!(f, "u64"),
            AnalysedType::Chr(_) => write!(f, "char"),
            AnalysedType::Result(type_result) => {
                let ok_type = type_result
                    .ok
                    .clone()
                    .map(|t| PrettyAnalysedType(t.deref().clone()))
                    .map_or("unknown".to_string(), |t| t.to_string());

                let error_type = type_result
                    .err
                    .clone()
                    .map(|t| PrettyAnalysedType(t.deref().clone()))
                    .map_or("unknown".to_string(), |t| t.to_string());

                write!(f, "Result<{}, {}>", ok_type, error_type)
            }
            AnalysedType::Option(t) => {
                let inner_type = PrettyAnalysedType(t.inner.deref().clone());
                write!(f, "Option<{}>", inner_type)
            }

            AnalysedType::Variant(type_variant) => {
                write!(f, "variant {{")?;
                for field in type_variant.cases.iter() {
                    let name = field.name.clone();
                    let typ = field.typ.clone();

                    match typ {
                        Some(t) => {
                            write!(f, "{}({}), ", name, PrettyAnalysedType(t))?;
                        }
                        None => {
                            write!(f, "{}, ", name)?;
                        }
                    }
                }
                write!(f, "}}")
            }
            AnalysedType::Enum(cases) => {
                write!(f, "enum {{")?;
                for case in cases.cases.iter() {
                    write!(f, "{}, ", case)?;
                }
                write!(f, "}}")
            }
            AnalysedType::Flags(flags) => {
                write!(f, "flags {{")?;
                for flag in flags.names.iter() {
                    write!(f, "{}, ", flag)?;
                }
                write!(f, "}}")
            }
            AnalysedType::Tuple(tuple) => {
                write!(f, "tuple<")?;
                for (index, typ) in tuple.items.iter().enumerate() {
                    write!(f, "{}", PrettyAnalysedType(typ.clone()))?;

                    if index < tuple.items.len() - 1 {
                        write!(f, ",")?;
                    }
                }
                write!(f, ">")
            }
            AnalysedType::List(list) => {
                write!(
                    f,
                    "list<{}>",
                    PrettyAnalysedType(list.inner.deref().clone())
                )
            }
            AnalysedType::Str(_) => {
                write!(f, "str")
            }
            AnalysedType::F64(_) => {
                write!(f, "f64")
            }
            AnalysedType::F32(_) => {
                write!(f, "f32")
            }
            AnalysedType::S64(_) => {
                write!(f, "s64")
            }
            AnalysedType::U32(_) => {
                write!(f, "u32")
            }
            AnalysedType::U16(_) => {
                write!(f, "u16")
            }
            AnalysedType::S16(_) => {
                write!(f, "s16")
            }
            AnalysedType::U8(_) => {
                write!(f, "u8")
            }
            AnalysedType::S8(_) => {
                write!(f, "s8")
            }
            AnalysedType::Bool(_) => {
                write!(f, "bool")
            }
            AnalysedType::Handle(_) => {
                write!(f, "handle<>")
            }
        }
    }
}
