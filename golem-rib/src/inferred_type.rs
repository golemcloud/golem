use std::collections::{HashMap, HashSet};

use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::*;
use crate::inferred_type::internal::sort_and_convert;

// TODO; Clean up Unification

// The reason to replicate analysed_type types
// in inferred_type can be explained with an example.
// During the type_pull_down stage
// we are yet unsure of a specific AnalysedType (it can be AllOf(...))
// yet for a specific field
// type, and yet be able to say that the root node is of the type record
// with the field name, and tag their types as InferredTypes.
#[derive(Debug, Hash, Clone, Eq, PartialEq, Encode, Decode)]
pub enum InferredType {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Chr,
    Str,
    List(Box<InferredType>),
    Tuple(Vec<InferredType>),
    Record(Vec<(String, InferredType)>),
    Flags(Vec<String>),
    Enum(Vec<String>),
    Option(Box<InferredType>),
    Result {
        ok: Option<Box<InferredType>>,
        error: Option<Box<InferredType>>,
    },
    Variant(Vec<(String, Option<InferredType>)>),
    Resource {
        resource_id: AnalysedResourceId,
        resource_mode: AnalysedResourceMode,
    },
    OneOf(Vec<InferredType>), // literalOneOf 1 --> u32 or u8?
    AllOf(Vec<InferredType>),
    Unknown,
    // Because function result can be a vector of types
    Sequence(Vec<InferredType>),
}

pub struct TypeErrorMessage(pub String);

impl InferredType {
    pub fn all_of(types: Vec<InferredType>) -> Option<InferredType> {
        if types.is_empty() {
            None
        } else if types.len() == 1 {
            types.into_iter().next()
        } else {
            Some(InferredType::AllOf(types))
        }
    }

    pub fn one_of(types: Vec<InferredType>) -> Option<InferredType> {
        if types.is_empty() {
            None
        } else if types.len() == 1 {
            types.into_iter().next()
        } else {
            Some(InferredType::OneOf(types))
        }
    }

    pub fn is_unit(&self) -> bool {
        match self {
            InferredType::Sequence(types) => types.is_empty(),
            _ => false,
        }
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self, InferredType::Unknown)
    }

    pub fn is_one_of(&self) -> bool {
        matches!(self, InferredType::OneOf(_))
    }
    pub fn un_resolved(&self) -> Option<String> {
        match self {
            InferredType::Bool => None,
            InferredType::S8 => None,
            InferredType::U8 => None,
            InferredType::S16 => None,
            InferredType::U16 => None,
            InferredType::S32 => None,
            InferredType::U32 => None,
            InferredType::S64 => None,
            InferredType::U64 => None,
            InferredType::F32 => None,
            InferredType::F64 => None,
            InferredType::Chr => None,
            InferredType::Str => None,
            InferredType::List(inferred_type) => inferred_type.un_resolved(),
            InferredType::Tuple(types) => {
                for typ in types {
                    if let Some(unresolved) = typ.un_resolved() {
                        return Some(unresolved);
                    }
                }
                None
            }
            InferredType::Record(field) => {
                for (field, typ) in field {
                    if let Some(unresolved) = typ.un_resolved() {
                        return Some(format!("Un-inferred type for field {} in record: {}", field, unresolved));
                    }
                }
                None
            }
            InferredType::Flags(_) => None,
            InferredType::Enum(_) => None,
            InferredType::Option(inferred_type) => {
                if let Some(unresolved) = inferred_type.un_resolved() {
                    return Some(unresolved);
                }
                None
            }
            InferredType::Result { ok, error } => {
                if let Some(ok) = ok {
                    if let Some(unresolved) = ok.un_resolved() {
                        return Some(unresolved);
                    }
                }
                if let Some(error) = error {
                    if let Some(unresolved) = error.un_resolved() {
                        return Some(unresolved);
                    }
                }
                None
            }
            InferredType::Variant(variant) => {
                for (_, typ) in variant {
                    if let Some(typ) = typ {
                        if let Some(unresolved) = typ.un_resolved() {
                            return Some(unresolved);
                        }
                    }
                }
                None
            }
            InferredType::Resource { .. } => None,
            InferredType::OneOf(possibilities) => {
                Some(format!("Cannot resolve {:?}", possibilities))
            }
            InferredType::AllOf(possibilities) => {
                Some(format!("Cannot be all of {:?}", possibilities))
            }
            InferredType::Unknown => Some("Unknown".to_string()),
            InferredType::Sequence(inferred_types) =>  {
                for typ in inferred_types {
                    if let Some(unresolved) = typ.un_resolved() {
                        return Some(unresolved);
                    }
                }
                None

            }
        }
    }

    pub fn unify_types_and_verify(&self) -> Result<InferredType, Vec<String>> {
        let unified = self.unify_types()?;
        if let Some(unresolved) = unified.un_resolved() {
            return Err(vec![unresolved]);
        }
        Ok(unified)
    }
    pub fn unify_types(&self) -> Result<InferredType, Vec<String>> {
        match self {
            // AllOf types may include AllOf Types and OneOf types within itself
            // Semantic reasoning is possible for  such a type only if group all the one-ofs together
            // within this list and flatten all the all-ofs allowing reasonable unification.
            // Example: AllOf(OneOf(u32, u8), AllOf(Str), OneOf(u16, u32)) is flattened
            // to AllOf(OneOf(u32, u8, u16, u32), Str)
            // and now this type cannot be unified and cannot pass since a Str is not one of the numbers
            InferredType::AllOf(types) => {
                let flattened_all_ofs = Self::flatten_all_of_list(types);
                Self::unify_all_required_types(&flattened_all_ofs)
            }

            // Unlike AllOf types, which hardly fails while unification, OneOfs is more prone to type check failures.as
            // Unifying OneOfs should return a proper type back instead of alternatives.
            // Example: There is no reason to the following
            // let x = Expr::Number(1, OneOf(U32, U64));
            // call(x) // expecting U32
            InferredType::OneOf(one_of_types) => {
                let flattened_one_ofs = Self::flatten_one_of_list(one_of_types);
                Self::unify_all_alternative_types(&flattened_one_ofs)
            }
            InferredType::Option(inner_type) => {
                let unified_inner_type = inner_type.unify_types()?;
                Ok(InferredType::Option(Box::new(unified_inner_type)))
            }

            InferredType::Result { ok, error } => {
                let unified_ok = match ok {
                    Some(ok) => Some(Box::new(ok.unify_types()?)),
                    None => None,
                };

                let unified_error = match error {
                    Some(error) => Some(Box::new(error.unify_types()?)),
                    None => None,
                };

                Ok(InferredType::Result {
                    ok: unified_ok,
                    error: unified_error,
                })
            }

            InferredType::Record(fields) => {
                let mut unified_fields = vec![];
                for (field, typ) in fields {
                    let unified_type = typ.unify_types()?;
                    unified_fields.push((field.clone(), unified_type));
                }
                Ok(InferredType::Record(unified_fields))
            }

            InferredType::Tuple(types) => {
                let mut unified_types = vec![];
                for typ in types {
                    let unified_type = typ.unify_types()?;
                    unified_types.push(unified_type);
                }
                Ok(InferredType::Tuple(unified_types))
            }

            InferredType::List(typ) => {
                let unified_type = typ.unify_types()?;
                Ok(InferredType::List(Box::new(unified_type)))
            }

            InferredType::Flags(flags) => Ok(InferredType::Flags(flags.clone())),

            InferredType::Enum(variants) => Ok(InferredType::Enum(variants.clone())),

            InferredType::Variant(variants) => {
                let mut unified_variants = vec![];
                for (variant, typ) in variants {
                    let unified_type = match typ {
                        Some(typ) => Some(Box::new(typ.unify_types()?)),
                        None => None,
                    };
                    unified_variants.push((variant.clone(), unified_type.as_deref().cloned()));
                }
                Ok(InferredType::Variant(unified_variants))
            }

            InferredType::Resource {
                resource_id,
                resource_mode,
            } => Ok(InferredType::Resource {
                resource_id: resource_id.clone(),
                resource_mode: resource_mode.clone(),
            }),

            _ => Ok(self.clone()),
        }
    }

    fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
        let mut one_of_types = vec![];
        let mut all_of_types = vec![];

        for typ in types {
            match typ {
                InferredType::OneOf(types) => {
                    let flattened = Self::flatten_one_of_list(types);
                    one_of_types.extend(flattened);
                }
                // we made sure to flatten all the all ofs
                InferredType::AllOf(all_of) => {
                    let flattened = Self::flatten_all_of_list(all_of);
                    all_of_types.extend(flattened);
                }
                _ => {
                    all_of_types.push(typ.clone());
                }
            }
        }

        if !one_of_types.is_empty() {
            all_of_types.extend(vec![InferredType::OneOf(one_of_types)]);
        }

        all_of_types
    }

    fn flatten_one_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
        let mut one_of_types = vec![];
        let mut all_of_types = vec![];

        for typ in types {
            match typ {
                InferredType::OneOf(types) => {
                    let flattened = Self::flatten_one_of_list(types);

                    one_of_types.extend(flattened);
                }
                // we made sure to flatten all the all ofs
                InferredType::AllOf(types) => {
                    let flattened = Self::flatten_all_of_list(types);
                    all_of_types.extend(flattened);
                }
                _ => {
                    one_of_types.push(typ.clone());
                }
            }
        }

        if !all_of_types.is_empty() {
            one_of_types.extend(vec![InferredType::AllOf(all_of_types)]);
        }

        one_of_types
    }

    fn all_numbers(types: &[InferredType]) -> bool {
        types.iter().all(|t| t.is_number())
    }

    fn unify_all_alternative_types(types: &Vec<InferredType>) -> Result<InferredType, Vec<String>> {
        let mut unified_type = InferredType::Unknown;

        let mut one_ofs = vec![];
        for typ in types {
            let unified = typ.unify_types().unwrap_or(typ.clone());
            match unified_type.unify_with_alternative(&unified) {
                Ok(t) => {
                    unified_type = t.clone();
                },
                Err(e) => {
                    if !unified_type.is_unknown() {
                        unified_type = InferredType::OneOf(Self::flatten_one_of_list(&vec![unified_type.clone(), unified.clone()]));
                    }
                    one_ofs.push(unified);
                }
            };
        }
        // This may or may not result in AllOf itself
        Ok(unified_type)
    }

    fn unify_all_required_types(types: &Vec<InferredType>) -> Result<InferredType, Vec<String>> {
        let mut unified_type = InferredType::Unknown;
        for typ in types {
            let unified = typ.unify_types().unwrap_or(typ.clone());
            unified_type = unified_type.unify_with_required(&unified)?;
        }
        // This may or may not result in AllOf itself
        Ok(unified_type)
    }

    // An example:
    // OneOf(Record("a" ->  Type A), Record("a" ->  Type B))
    // a field exist on both sides, and if Type A != Type B, they couldn't be merged
    // However, if it says
    // OneOf(Record("a" -> AllOf(OneOf(TypeA, TypeB), TypeA), Record("a" -> TypeA))
    // these could be merged, since the types merge to TypeA on both sides
    fn unify_with_alternative(&self, other: &InferredType) -> Result<InferredType, Vec<String>> {
        if self == &InferredType::Unknown {
            Ok(other.clone())
        } else if other.is_unknown()|| self == other {
            Ok(self.clone())
        } else {
            match (self, other) {
                (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                    if a_fields.len() != b_fields.len() {
                        return Err(vec!["Record fields do not match".to_string()]);
                    }

                    let mut fields = a_fields.clone();

                    for (field, typ) in fields.iter_mut() {
                        if let Some((_, b_type)) =
                            b_fields.iter().find(|(b_field, _)| b_field == field)
                        {
                            let unified_b_type = b_type.unify_types()?;
                            let unified_a_type = typ.unify_types()?;
                            if unified_a_type == unified_b_type {
                                *typ = unified_a_type
                            } else {
                                return Err(vec!["Record fields do not match".to_string()]);
                            }
                        } else {
                            return Err(vec!["Record fields do not match".to_string()]);
                        }
                    }

                    Ok(InferredType::Record(fields))
                }
                (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                    if a_types.len() != b_types.len() {
                        return Err(vec!["Tuple lengths do not match".to_string()]);
                    }

                    let mut types = a_types.clone();

                    for (a_type, b_type) in types.iter_mut().zip(b_types) {
                        let unified_b_type = b_type.unify_types()?;
                        let unified_a_type = a_type.unify_types()?;
                        if unified_a_type == unified_b_type {
                            *a_type = unified_a_type
                        } else {
                            return Err(vec!["Record fields do not match".to_string()]);
                        }
                    }

                    Ok(InferredType::Tuple(types))
                }

                (InferredType::List(a_type), InferredType::List(b_type)) => {
                    let unified_b_type = b_type.unify_types()?;
                    let unified_a_type = a_type.unify_types()?;
                    if unified_a_type == unified_b_type {
                        Ok(InferredType::List(Box::new(unified_a_type)))
                    } else {
                        Err(vec!["Record fields do not match".to_string()])
                    }
                }

                (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                    // Semantics of alternative for a flag is, pick the one with the largest size
                    // This is again giving users more flexibility with flags literals without the need to call a worker function
                    // Also, it is impossible to pick and choose flags from both sides since the order of flags is important
                    // at wasm side when calling a worker function, as they get converted to a vector of booleans zipped
                    // with the actual flag names
                    if a_flags.len() >= b_flags.len() {
                        Ok(InferredType::Flags(a_flags.clone()))
                    } else {
                        Ok(InferredType::Flags(b_flags.clone()))
                    }
                }

                (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                    if a_variants == b_variants {
                        Ok(InferredType::Enum(a_variants.clone()))
                    } else {
                        Err(vec!["Enum variants do not match".to_string()])
                    }
                }

                (InferredType::Option(a_type), InferredType::Option(b_type)) => {
                    let unified_b_type = b_type.unify_types()?;
                    let unified_a_type = a_type.unify_types()?;
                    let combined = unified_a_type.unify_with_alternative(&unified_b_type)?;
                    Ok(InferredType::Option(Box::new(combined)))

                }

                (
                    InferredType::Result {
                        ok: a_ok,
                        error: a_error,
                    },
                    InferredType::Result {
                        ok: b_ok,
                        error: b_error,
                    },
                ) => {
                    let unified_b_ok = match (a_ok, b_ok) {
                        (Some(a_inner), Some(b_inner)) => {
                            let unified_b_inner = b_inner.unify_types()?;
                            let unified_a_inner = a_inner.unify_types()?;
                            if unified_a_inner == unified_b_inner {
                                Some(Box::new(unified_a_inner))
                            } else {
                                return Err(vec!["Record fields do not match".to_string()]);
                            }
                        }
                        (None, None) => None,
                        (Some(ok), None) => Some(Box::new(*ok.clone())),
                        (None, Some(ok)) => Some(Box::new(*ok.clone())),
                    };

                    let unified_b_error = match (a_error, b_error) {
                        (Some(a_inner), Some(b_inner)) => {
                            let unified_b_inner = b_inner.unify_types()?;
                            let unified_a_inner = a_inner.unify_types()?;
                            if unified_a_inner == unified_b_inner {
                                Some(Box::new(unified_a_inner))
                            } else {
                                return Err(vec!["Record fields do not match".to_string()]);
                            }
                        }
                        (None, None) => None,
                        (Some(ok), None) => Some(Box::new(*ok.clone())),
                        (None, Some(ok)) => Some(Box::new(*ok.clone())),
                    };

                    Ok(InferredType::Result {
                        ok: unified_b_ok,
                        error: unified_b_error,
                    })
                }

                // We hardly reach a situation where a variant can be OneOf, but if we happen to encounter this
                // the only way to merge them is to make sure all the variants types are matching
                (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
                    if a_variants.len() != b_variants.len() {
                        return Err(vec!["Variant fields do not match".to_string()]);
                    }

                    let mut variants = a_variants.clone();

                    for (variant, a_type) in variants.iter_mut() {
                        if let Some((_, b_type)) = b_variants
                            .iter()
                            .find(|(b_variant, _)| b_variant == variant)
                        {
                            let x = match b_type {
                                Some(x) => Some(x.unify_types()?),
                                None => None,
                            };

                            let y = match a_type {
                                Some(y) => Some(y.unify_types()?),
                                None => None,
                            };
                            if x == y {
                                *a_type = x
                            } else {
                                return Err(vec!["Variant fields do not match".to_string()]);
                            }
                        } else {
                            return Err(vec!["Variant fields do not match".to_string()]);
                        }
                    }

                    Ok(InferredType::Variant(variants))
                }

                // We shouldn't get into a situation where we have OneOf 2 different resource handles.
                // The only possibility of unification there is only if they match exact
                (
                    InferredType::Resource {
                        resource_id: a_id,
                        resource_mode: a_mode,
                    },
                    InferredType::Resource {
                        resource_id: b_id,
                        resource_mode: b_mode,
                    },
                ) => {
                    if a_id == b_id && a_mode == b_mode {
                        Ok(InferredType::Resource {
                            resource_id: a_id.clone(),
                            resource_mode: a_mode.clone(),
                        })
                    } else {
                        Err(vec!["Resource id or mode do not match".to_string()])
                    }
                }

                (InferredType::AllOf(a_types), inferred_types) => {
                    let unified_all_types = Self::unify_all_required_types(a_types)?;
                    let alternative_type = inferred_types.unify_types()?;

                    if unified_all_types == alternative_type {
                        Ok(unified_all_types)
                    } else {
                        Err(vec!["AllOf types do not match".to_string()])
                    }
                }

                (inferred_types, InferredType::AllOf(b_types)) => {
                    let unified_all_types = Self::unify_all_required_types(b_types)?;
                    let alternative_type = inferred_types.unify_types()?;

                    if unified_all_types == alternative_type {
                        Ok(unified_all_types)
                    } else {
                        Err(vec!["AllOf types do not match".to_string()])
                    }
                }

                // In all other cases, it should match exact
                (a, b) => {
                    if a == b {
                        Ok(a.clone())
                    } else {
                        dbg!("here?");
                        Err(vec![format!(
                            "Types do not match. Inferred to be both {:?} and {:?}",
                            a, b
                        )])
                    }
                }
            }
        }
    }

    // Unify types where both types do matter. Example in reality x can form to be both U64 and U32 in the IR, resulting in AllOf
    // Result of this type hardly becomes OneOf
    fn unify_with_required(&self, other: &InferredType) -> Result<InferredType, Vec<String>> {
        if other.is_unknown() {
            self.unify_types()
        } else if self.is_unknown() {
            other.unify_types()
        } else if self == other {
            self.unify_types()
        } else {
            match (self, other) {
                // { name: Opt<Str>, age: Option<U32> } and { name: Str }
                (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                    let mut fields: HashMap<String, InferredType> = HashMap::new();
                    // Common fields unified else kept it as it is
                    for (a_name, a_type) in a_fields {
                        if let Some((_, b_type)) =
                            b_fields.iter().find(|(b_name, _)| b_name == a_name)
                        {
                            fields.insert(a_name.clone(), a_type.unify_with_required(b_type)?);
                        } else {
                            fields.insert(a_name.clone(), a_type.clone());
                        }
                    }

                    for (a_name, a_type) in b_fields {
                        if !a_fields.iter().any(|(b_name, _)| b_name == a_name) {
                            fields.insert(a_name.clone(), a_type.clone());
                        }
                    }

                    Ok(InferredType::Record(internal::sort_and_convert(fields)))
                }
                (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                    if a_types.len() != b_types.len() {
                        return Err(vec!["Tuple lengths do not match".to_string()]);
                    }
                    let mut types = Vec::new();
                    for (a_type, b_type) in a_types.iter().zip(b_types) {
                        types.push(a_type.unify_with_required(b_type)?);
                    }
                    Ok(InferredType::Tuple(types))
                }
                (InferredType::List(a_type), InferredType::List(b_type)) => Ok(InferredType::List(
                    Box::new(a_type.unify_with_required(b_type)?),
                )),
                (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                    // It's hard to unify flags. Or the semantic meaning of unifying flags is hard
                    // so we simply expect them to be the same, as the ProtoVal expects a vector of boolean
                    // in the correct order when invoking worker function with flags. Unifying them has
                    // no guarantee it's in the right order
                    if a_flags != b_flags {
                        return Err(vec!["Flags do not match".to_string()]);
                    }
                    Ok(InferredType::Flags(a_flags.clone()))
                }
                // It's hard to unify flags. Or the semantic meaning of unifying flags is hard
                // so we simply expect them to be the same, as the ProtoVal expects a vector of boolean
                // in the correct order when invoking worker function with flags. Unifying them has
                // no guarantee it's in the right order. Also most probably enums and flags are derived
                // from component metadata and type inference shouldn't need to deal with dynamically created
                // enum strings
                (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                    if a_variants != b_variants {
                        return Err(vec!["Enum variants do not match".to_string()]);
                    }
                    Ok(InferredType::Enum(a_variants.clone()))
                }
                (InferredType::Option(a_type), InferredType::Option(b_type)) => Ok(
                    InferredType::Option(Box::new(a_type.unify_with_required(b_type)?)),
                ),

                (InferredType::Option(a_type), inferred_type) => {
                    let unified_left = a_type.unify_types()?;
                    let unified_right = inferred_type.unify_types()?;
                    let combined = unified_left.unify_with_required(&unified_right)?;
                    Ok(InferredType::Option(Box::new(combined)))
                }
                (inferred_type, InferredType::Option(a_type)) => {
                    let unified_left = a_type.unify_types()?;
                    let unified_right = inferred_type.unify_types()?;
                    let combined = unified_left.unify_with_required(&unified_right)?;
                    Ok(InferredType::Option(Box::new(combined)))
                }

                (
                    InferredType::Result {
                        ok: a_ok,
                        error: a_error,
                    },
                    InferredType::Result {
                        ok: b_ok,
                        error: b_error,
                    },
                ) => {
                    // here we basically replace the ones that are empty with the other that is non empty
                    let ok = match (a_ok, b_ok) {
                        (Some(a_inner), Some(b_inner)) => {
                            Some(Box::new(a_inner.unify_with_required(b_inner)?))
                        }
                        (None, None) => None,
                        (Some(ok), None) => Some(Box::new(*ok.clone())),
                        (None, Some(ok)) => Some(Box::new(*ok.clone())),
                    };

                    let error = match (a_error, b_error) {
                        (Some(a_inner), Some(b_inner)) => {
                            Some(Box::new(a_inner.unify_with_required(b_inner)?))
                        }
                        (None, None) => None,
                        (Some(ok), None) => Some(Box::new(*ok.clone())),
                        (None, Some(ok)) => Some(Box::new(*ok.clone())),
                    };
                    Ok(InferredType::Result { ok, error })
                }
                // Here we make sure we unify the types but pick just one side of the variant
                // There can be changes in this logic but depends on the test cases
                (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
                    let mut variants = HashMap::new();
                    for (a_name, a_type) in a_variants {
                        if let Some((_, b_type)) =
                            b_variants.iter().find(|(b_name, _)| b_name == a_name)
                        {
                            let unified_type = match (a_type, b_type) {
                                (Some(a_inner), Some(b_inner)) => {
                                    Some(Box::new(a_inner.unify_with_required(b_inner)?))
                                }
                                (None, None) => None,
                                (Some(_), None) => None,
                                (None, Some(_)) => None,
                            };
                            variants.insert(a_name.clone(), unified_type);
                        }
                    }
                    Ok(InferredType::Variant(
                        variants
                            .iter()
                            .map(|(n, t)| (n.clone(), t.clone().map(|v| *v)))
                            .collect(),
                    ))
                }
                (
                    InferredType::Resource {
                        resource_id: a_id,
                        resource_mode: a_mode,
                    },
                    InferredType::Resource {
                        resource_id: b_id,
                        resource_mode: b_mode,
                    },
                ) => {
                    if a_id != b_id || a_mode != b_mode {
                        return Err(vec!["Resource id or mode do not match".to_string()]);
                    }
                    Ok(InferredType::Resource {
                        resource_id: a_id.clone(),
                        resource_mode: a_mode.clone(),
                    })
                }

                // Given we always flatten AllOf and OneOf, in reality we can check if All of the types are part of the One ofs.
                (InferredType::AllOf(types), InferredType::OneOf(one_of_types)) => {
                    for typ in types {
                        if !one_of_types.contains(typ) {
                            return Err(
                                vec!["AllOf types are not part of OneOf types".to_string()],
                            );
                        }
                    }
                    // Once we know the types in the AllOf are part of OneOf, we can simply return the unified all-of
                    Self::unify_all_required_types(types)
                }

                // Given we always flatten AllOf and OneOf, in reality we can check if All of the types are part of the One ofs.
                (InferredType::OneOf(one_of_types), InferredType::AllOf(all_of_types)) => {
                    for required_type in all_of_types {
                        if !one_of_types.contains(required_type) {
                            return Err(
                                vec!["OneOf types are not part of AllOf types".to_string()],
                            );
                        }
                    }
                    Self::unify_all_required_types(all_of_types)
                }

                (InferredType::OneOf(types), inferred_type) => {
                    if types.contains(inferred_type) {
                        Ok(inferred_type.clone())
                    } else {
                        let type_set: HashSet<_> = types.iter().collect::<HashSet<_>>();
                        Err(vec![format!("Types do not match. Inferred to be any of {:?}, but found (or used as) {:?} ",  type_set, inferred_type)])
                    }
                }

                (inferred_type, InferredType::OneOf(types)) => {
                    if types.contains(inferred_type) {
                        Ok(inferred_type.clone())
                    } else {
                        let type_set: HashSet<_> = types.iter().collect::<HashSet<_>>();

                        Err(vec![format!("Types do not match. Inferred to be any of {:?}, but found or used as {:?} ", type_set, inferred_type)])
                    }
                }

                (InferredType::AllOf(types), inferred_type) => {
                    let x =
                        types.iter().filter(|x| !x.is_unknown()).map(|t| t.unify_with_required(inferred_type).unwrap()).collect::<Vec<_>>();

                    Self::unify_all_required_types(&x)

                }

                (inferred_type, InferredType::AllOf(types)) => {
                    let result = InferredType::AllOf(types.clone()).unify_types()?;

                    result.unify_with_required(inferred_type)
                }

                (inferred_type1, inferred_type2) => {
                    if inferred_type1 == inferred_type2 {
                        Ok(inferred_type1.clone())
                    } else if inferred_type1.is_number() && inferred_type2.is_number() {
                        Ok(InferredType::AllOf(vec![
                            inferred_type1.clone(),
                            inferred_type2.clone(),
                        ]))
                    } else {
                        Err(vec![format!(
                            "Types do not match. Inferred to be both {:?} and {:?}",
                            inferred_type1, inferred_type2
                        )])
                    }
                }
            }
        }
    }

    pub fn type_check(&self) -> Result<(), Vec<TypeErrorMessage>> {
        let mut errors = Vec::new();

        match self {
            InferredType::AllOf(types) => {
                if !self.check_all_compatible(types) {
                    errors.push(TypeErrorMessage(format!("Incompatible types: {:?}", self)));
                }
            }
            InferredType::OneOf(inferred_type) => {
                if !inferred_type.iter().all(|t| t.is_number()) {
                    errors.push(TypeErrorMessage(format!("Ambiguous type {:?}", self)));
                }
            }
            // Sequence is a special case, and we don't expect them to be compatible
            _ => {}
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
    fn is_number(&self) -> bool {
        matches!(
            self,
            InferredType::S8
                | InferredType::U8
                | InferredType::S16
                | InferredType::U16
                | InferredType::S32
                | InferredType::U32
                | InferredType::S64
                | InferredType::U64
                | InferredType::F32
                | InferredType::F64
        )
    }

    fn is_string(&self) -> bool {
        matches!(self, InferredType::Str)
    }

    fn check_all_compatible(&self, types: &[InferredType]) -> bool {
        if types.len() > 1 {
            for i in 0..types.len() {
                for j in (i + 1)..types.len() {
                    if !Self::are_compatible(&types[i], &types[j]) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn are_compatible(a: &InferredType, b: &InferredType) -> bool {
        match (a, b) {
            (InferredType::Unknown, _) | (_, InferredType::Unknown) => true,

            (InferredType::List(a_type), InferredType::List(b_type)) => {
                Self::are_compatible(a_type, b_type)
            }

            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                if a_types.len() != b_types.len() {
                    return false;
                }
                for (a_type, b_type) in a_types.iter().zip(b_types) {
                    if !Self::are_compatible(a_type, b_type) {
                        return false;
                    }
                }
                true
            }

            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        if !Self::are_compatible(a_type, b_type) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            }

            (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => a_flags == b_flags,

            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                a_variants == b_variants
            }

            (InferredType::Option(a_type), InferredType::Option(b_type)) => {
                Self::are_compatible(a_type, b_type)
            }

            (
                InferredType::Result {
                    ok: a_ok,
                    error: a_error,
                },
                InferredType::Result {
                    ok: b_ok,
                    error: b_error,
                },
            ) => {
                let ok = match (a_ok, b_ok) {
                    (Some(a_inner), Some(b_inner)) => Self::are_compatible(a_inner, b_inner),
                    (None, None) => true,
                    (Some(_), None) => true,
                    (None, Some(_)) => true,
                };
                let error = match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => Self::are_compatible(a_inner, b_inner),
                    (None, None) => true,
                    (Some(_), None) => true,
                    (None, Some(_)) => true,
                };

                ok && error
            }

            (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
                for (a_name, a_type) in a_variants {
                    if let Some((_, b_type)) =
                        b_variants.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        match (a_type, b_type) {
                            (Some(a_inner), Some(b_inner)) => {
                                if !Self::are_compatible(a_inner, b_inner) {
                                    return false;
                                }
                            }
                            (None, None) => {}
                            _ => return false,
                        }
                    } else {
                        return false;
                    }
                }
                true
            }

            (
                InferredType::Resource {
                    resource_id: a_id,
                    resource_mode: a_mode,
                },
                InferredType::Resource {
                    resource_id: b_id,
                    resource_mode: b_mode,
                },
            ) => a_id == b_id && a_mode == b_mode,

            (InferredType::OneOf(types), InferredType::AllOf(typ)) => {
                for t in typ {
                    if !types.contains(t) {
                        return false;
                    }
                }

                true
            }

            (InferredType::AllOf(types), InferredType::OneOf(typ)) => {
                for t in typ {
                    if !types.contains(t) {
                        return false;
                    }
                }

                true
            }

            (InferredType::AllOf(types), inferred_type) => {
                for t in types {
                    if !Self::are_compatible(t, inferred_type) {
                        return false;
                    }
                }
                true
            }

            (inferred_type, InferredType::AllOf(types)) => {
                for t in types {
                    if !Self::are_compatible(inferred_type, t) {
                        return false;
                    }
                }
                true
            }

            (InferredType::OneOf(types), inferred_type) => types.contains(inferred_type),

            (inferred_type, InferredType::OneOf(types)) => types.contains(inferred_type),

            (a, b) => a.is_number() && b.is_number() || a.is_string() && b.is_string(),
        }
    }

    // The only to update inferred type is to discard unknown types
    // and push that as `allOf`
    pub fn update(&mut self, new_inferred_type: InferredType) {
        if !internal::need_update(self, &new_inferred_type) {
            return;
        }

        match (&mut *self, new_inferred_type) {
            (InferredType::Unknown, new_type) => {
                *self = new_type;
            }

            (InferredType::AllOf(existing_types), InferredType::AllOf(mut new_types)) => {
                // Extend with new types only if they are not already present
                new_types.retain(|new_type| !existing_types.contains(new_type) || !new_type.is_unknown());
                existing_types.retain(|x| !x.is_unknown());
                existing_types.extend(new_types);
            }

            (InferredType::AllOf(existing_types), new_type) => {
                if !existing_types.contains(&new_type) && !new_type.is_unknown() {
                    existing_types.retain(|x| !x.is_unknown());
                    existing_types.push(new_type);
                }
            }

            (current_type, InferredType::AllOf(mut new_types)) => {
                new_types.retain(|x| !x.is_unknown());

                if new_types.contains(current_type) || current_type.is_unknown() {
                    *current_type = InferredType::AllOf(new_types);
                } else {
                    if current_type.is_unknown() {
                        *current_type = InferredType::AllOf(new_types);
                    } else {
                        new_types.push(current_type.clone());
                        *current_type = InferredType::AllOf(new_types);
                    }
                }
            }

            (InferredType::OneOf(existing_types), InferredType::OneOf(mut new_types)) => {
                // Extend with new types only if they are not already present
                new_types.retain(|new_type| !existing_types.contains(new_type));
                existing_types.extend(new_types);
            }

            (InferredType::OneOf(types), new_type) => {
                types.retain(|x| !x.is_unknown());
                *self = InferredType::AllOf(vec![self.clone(), new_type]);
            }

            (current_type, InferredType::OneOf(mut newtypes)) => {
                newtypes.retain(|x| !x.is_unknown());
                if !current_type.is_unknown() {
                    *self = InferredType::AllOf(vec![current_type.clone(), InferredType::OneOf(newtypes)]);
                } else {
                    *self = InferredType::OneOf(newtypes);
                }
            }

            (current_type, new_type) => {
                if current_type != &new_type && !current_type.is_unknown() {
                    *current_type = InferredType::AllOf(vec![current_type.clone(), new_type]);
                } else {
                    *current_type = new_type;
                }
            }
        }
    }



}

impl From<AnalysedType> for InferredType {
    fn from(analysed_type: AnalysedType) -> Self {
        match analysed_type {
            AnalysedType::Bool(_) => InferredType::Bool,
            AnalysedType::S8(_) => InferredType::S8,
            AnalysedType::U8(_) => InferredType::U8,
            AnalysedType::S16(_) => InferredType::S16,
            AnalysedType::U16(_) => InferredType::U16,
            AnalysedType::S32(_) => InferredType::S32,
            AnalysedType::U32(_) => InferredType::U32,
            AnalysedType::S64(_) => InferredType::S64,
            AnalysedType::U64(_) => InferredType::U64,
            AnalysedType::F32(_) => InferredType::F32,
            AnalysedType::F64(_) => InferredType::F64,
            AnalysedType::Chr(_) => InferredType::Chr,
            AnalysedType::Str(_) => InferredType::Str,
            AnalysedType::List(t) => InferredType::List(Box::new((*t.inner).into())),
            AnalysedType::Tuple(ts) => {
                InferredType::Tuple(ts.items.into_iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => InferredType::Record(
                fs.fields
                    .into_iter()
                    .map(|name_type| (name_type.name, name_type.typ.into()))
                    .collect(),
            ),
            AnalysedType::Flags(vs) => InferredType::Flags(vs.names),
            AnalysedType::Enum(vs) => InferredType::Enum(vs.cases),
            AnalysedType::Option(t) => InferredType::Option(Box::new((*t.inner).into())),
            AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err, .. }) => {
                InferredType::Result {
                    ok: ok.map(|t| Box::new((*t).into())),
                    error: err.map(|t| Box::new((*t).into())),
                }
            }
            AnalysedType::Variant(vs) => InferredType::Variant(
                vs.cases
                    .into_iter()
                    .map(|name_type_pair| {
                        (name_type_pair.name, name_type_pair.typ.map(|t| t.into()))
                    })
                    .collect(),
            ),
            AnalysedType::Handle(golem_wasm_ast::analysis::TypeHandle { resource_id, mode }) => {
                InferredType::Resource {
                    resource_id,
                    resource_mode: mode,
                }
            }
        }
    }
}

mod internal {
    use crate::InferredType;
    use std::collections::HashMap;

    pub(crate) fn need_update(
        current_inferred_type: &InferredType,
        new_inferred_type: &InferredType,
    ) -> bool {
        current_inferred_type != new_inferred_type && !new_inferred_type.is_unknown()
    }

    pub(crate) fn sort_and_convert(
        hashmap: HashMap<String, InferredType>,
    ) -> Vec<(String, InferredType)> {
        let mut vec: Vec<(String, InferredType)> = hashmap.into_iter().collect(); // Step 1: Collect into Vec
        vec.sort_by(|a, b| a.0.cmp(&b.0)); // Step 2: Sort by String keys
        vec // Step 3: Return sorted Vec
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_flatte_one_of() {
        use super::InferredType;
        let one_of = vec![
            InferredType::U8,
            InferredType::U16,
            InferredType::U32,
            InferredType::OneOf(vec![   InferredType::U8,
                                        InferredType::U16,
                                        InferredType::U32, InferredType::AllOf(vec![InferredType::U64, InferredType::OneOf(vec![InferredType::U64, InferredType::U8])])]),
        ];

        let flattened = InferredType::flatten_one_of_list(&one_of);
        dbg!(flattened);
        assert!(false)
    }
}