use crate::Expr;
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::rib::RibInputType as ProtoRibInputType;
use golem_wasm_ast::analysis::{AnalysedType, TypeStr};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct RibInputTypeInfo {
    pub types: HashMap<String, AnalysedType>,
}
impl RibInputTypeInfo {
    pub fn empty() -> Self {
        RibInputTypeInfo {
            types: HashMap::new(),
        }
    }

    pub fn from_expr(expr: &mut Expr) -> Result<RibInputTypeInfo, String> {
        let mut queue = VecDeque::new();

        let mut global_variables = HashMap::new();

        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    if variable_id.is_global() {
                        let analysed_type = AnalysedType::try_from(&*inferred_type)?;
                        global_variables.insert(variable_id.name(), analysed_type);
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        Ok(RibInputTypeInfo {
            types: global_variables,
        })
    }

    pub fn from_pure_expr(
        expr: &mut Expr,
        alternative_for_uninferred_type: AnalysedType,
    ) -> RibInputTypeInfo {
        let mut queue = VecDeque::new();

        let mut global_variables = HashMap::new();

        queue.push_back(expr);

        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Identifier(variable_id, inferred_type) => {
                    if variable_id.is_global() {
                        let analysed_type = internal::to_analysed_type_with_fallback(
                            inferred_type,
                            alternative_for_uninferred_type.clone(),
                        );

                        global_variables.insert(variable_id.name(), analysed_type);
                    }
                }
                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        RibInputTypeInfo {
            types: global_variables,
        }
    }
}

impl TryFrom<ProtoRibInputType> for RibInputTypeInfo {
    type Error = String;
    fn try_from(value: ProtoRibInputType) -> Result<Self, String> {
        let mut types = HashMap::new();
        for (key, value) in value.types {
            types.insert(key, AnalysedType::try_from(&value)?);
        }
        Ok(RibInputTypeInfo { types })
    }
}

impl From<RibInputTypeInfo> for ProtoRibInputType {
    fn from(value: RibInputTypeInfo) -> Self {
        let mut types = HashMap::new();
        for (key, value) in value.types {
            types.insert(key, golem_wasm_ast::analysis::protobuf::Type::from(&value));
        }
        ProtoRibInputType { types }
    }
}

mod internal {
    use crate::InferredType;
    use golem_wasm_ast::analysis::*;

    // To handle special cases: mainly to support ambiguous expressions such as request.path.user.id, where it can infer only
    // to be a record but not the type of id
    pub(crate) fn to_analysed_type_with_fallback(
        inferred_type: &InferredType,
        fallback: AnalysedType,
    ) -> AnalysedType {
        match inferred_type {
            InferredType::Bool => AnalysedType::Bool(TypeBool),
            InferredType::S8 => AnalysedType::S8(TypeS8),
            InferredType::U8 => AnalysedType::U8(TypeU8),
            InferredType::S16 => AnalysedType::S16(TypeS16),
            InferredType::U16 => AnalysedType::U16(TypeU16),
            InferredType::S32 => AnalysedType::S32(TypeS32),
            InferredType::U32 => AnalysedType::U32(TypeU32),
            InferredType::S64 => AnalysedType::S64(TypeS64),
            InferredType::U64 => AnalysedType::U64(TypeU64),
            InferredType::F32 => AnalysedType::F32(TypeF32),
            InferredType::F64 => AnalysedType::F64(TypeF64),
            InferredType::Chr => AnalysedType::Chr(TypeChr),
            InferredType::Str => AnalysedType::Str(TypeStr),
            InferredType::List(inferred_type) => AnalysedType::List(TypeList {
                inner: Box::new(to_analysed_type_with_fallback(
                    inferred_type.as_ref(),
                    fallback.clone(),
                )),
            }),
            InferredType::Tuple(tuple) => AnalysedType::Tuple(TypeTuple {
                items: tuple
                    .into_iter()
                    .map(|t| to_analysed_type_with_fallback(t, fallback.clone()))
                    .collect::<Vec<AnalysedType>>(),
            }),
            InferredType::Record(record) => AnalysedType::Record(TypeRecord {
                fields: record
                    .into_iter()
                    .map(|(name, typ)| NameTypePair {
                        name: name.to_string(),
                        typ: to_analysed_type_with_fallback(typ, fallback.clone()),
                    })
                    .collect::<Vec<NameTypePair>>(),
            }),
            InferredType::Flags(flags) => AnalysedType::Flags(TypeFlags {
                names: flags.clone(),
            }),
            InferredType::Enum(enums) => AnalysedType::Enum(TypeEnum {
                cases: enums.clone(),
            }),
            InferredType::Option(option) => AnalysedType::Option(TypeOption {
                inner: Box::new(to_analysed_type_with_fallback(
                    option.as_ref(),
                    fallback.clone(),
                )),
            }),
            InferredType::Result { ok, error } =>
            // In the case of result, there are instances users give just 1 value with zero function calls, we need to be flexible here
            {
                AnalysedType::Result(TypeResult {
                    ok: ok
                        .as_ref()
                        .map(|t| to_analysed_type_with_fallback(t.as_ref(), fallback.clone()))
                        .map(Box::new),
                    err: error
                        .as_ref()
                        .map(|t| to_analysed_type_with_fallback(t.as_ref(), fallback.clone()))
                        .map(Box::new),
                })
            }
            InferredType::Variant(variant) => AnalysedType::Variant(TypeVariant {
                cases: variant
                    .into_iter()
                    .map(|(name, typ)| NameOptionTypePair {
                        name: name.clone(),
                        typ: typ
                            .as_ref()
                            .map(|t| to_analysed_type_with_fallback(t, fallback.clone())),
                    })
                    .collect::<Vec<NameOptionTypePair>>(),
            }),
            InferredType::Resource {
                resource_id,
                resource_mode,
            } => AnalysedType::Handle(TypeHandle {
                resource_id: resource_id.clone(),
                mode: resource_mode.clone(),
            }),

            InferredType::OneOf(_) => fallback,
            InferredType::AllOf(types) => {
                if types.is_empty() {
                    fallback
                } else {
                    match types.first() {
                        Some(first) => to_analysed_type_with_fallback(first, fallback),
                        None => fallback,
                    }
                }
            }

            InferredType::Unknown => fallback,
            // We don't expect to have a sequence type in the inferred type.as
            // This implies Rib will not support multiple types from worker-function results
            InferredType::Sequence(vec) => {
                if vec.is_empty() {
                    fallback
                } else {
                    match vec.first() {
                        Some(first) => to_analysed_type_with_fallback(first, fallback),
                        None => fallback,
                    }
                }
            }
        }
    }
}
