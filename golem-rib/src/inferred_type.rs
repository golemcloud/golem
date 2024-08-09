use std::collections::HashMap;

use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::{AnalysedType};
use golem_wasm_rpc::protobuf::{TypedHandle, TypedResult};

// The reason to replicate analysed_type types
// in inferred_type can be explained with an example.
// During the type_pull_down stage
// we are yet unsure of a specific AnalysedType (it can be AllOf(...))
// yet for a specific field
// type, and yet be able to say that the root node is of the type record
// with the field name, and tag their types as InferredTypes.
#[derive(Debug, Clone, PartialEq, Encode, Decode)]
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
        resource_id: u64,
        resource_mode: i32,
    },
    OneOf(Vec<InferredType>), // literalOneOf 1 --> u32 or u8?
    AllOf(Vec<InferredType>),
    Unknown,
    // Because function result can be a vector of types
    Sequence(Vec<InferredType>),
}

struct TypeErrorMessage(String);

impl InferredType {
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
            _ => Ok(self.clone()),
        }
    }

    fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
        let mut one_of_types = vec![];
        let mut all_of_types = vec![];

        for typ in types {
            match typ {
                InferredType::OneOf(types) => {
                    let flattened = Self::flatten_one_of_list(&types);
                    one_of_types.extend(flattened);
                }
                // we made sure to flatten all the all ofs
                InferredType::AllOf(all_of) => {
                    let flattened = Self::flatten_all_of_list(&all_of);
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
                InferredType::OneOf(one_of) => {
                    let flattened = Self::flatten_one_of_list(&types);
                    one_of_types.extend(flattened);
                }
                // we made sure to flatten all the all ofs
                InferredType::AllOf(all_of) => {
                    let flattened = Self::flatten_all_of_list(&types);
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

    fn unify_all_alternative_types(types: &Vec<InferredType>) -> Result<InferredType, Vec<String>> {
        let mut unified_type = InferredType::Unknown;
        for typ in types {
            unified_type.update(typ.unify_with_alternative(&typ)?);
        }
        // This may or may not result in AllOf itself
        Ok(unified_type)
    }

    fn unify_all_required_types(types: &Vec<InferredType>) -> Result<InferredType, Vec<String>> {
        let mut unified_type = InferredType::Unknown;
        for typ in types {
            unified_type.update(typ.unify_with_required(&typ)?);
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
        match (self, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                if a_fields.len() != b_fields.len() {
                    return Err("Record fields do not match".to_string());
                }

                let mut fields = a_fields.clone();

                for (field, typ) in fields.iter_mut() {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_field, _)| b_field == field)
                    {
                        let unified_b_type = b_type.unify_types()?;
                        let unified_a_type = typ.unify_types()?;
                        if unified_a_type == unified_b_type {
                            *typ = unified_a_type
                        } else {
                            return Err("Record fields do not match".to_string());
                        }
                    } else {
                        return Err("Record fields do not match".to_string());
                    }
                }

                Ok(InferredType::Record(fields))
            }
            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                if a_types.len() != b_types.len() {
                    return Err("Tuple lengths do not match".to_string());
                }

                let mut types = a_types.clone();

                for (a_type, b_type) in types.iter_mut().zip(b_types) {
                    let unified_b_type = b_type.unify_types()?;
                    let unified_a_type = a_type.unify_types()?;
                    if unified_a_type == unified_b_type {
                        *a_type = unified_a_type
                    } else {
                        return Err("Record fields do not match".to_string());
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
                    Err("Record fields do not match".to_string())
                }
            }

            (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                if a_flags == b_flags {
                    Ok(InferredType::Flags(a_flags.clone()))
                } else {
                    Err("Flags do not match".to_string())
                }
            }

            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                if a_variants == b_variants {
                    Ok(InferredType::Enum(a_variants.clone()))
                } else {
                    Err("Enum variants do not match".to_string())
                }
            }

            (InferredType::Option(a_type), InferredType::Option(b_type)) => {
                let unified_b_type = b_type.unify_types()?;
                let unified_a_type = a_type.unify_types()?;
                if unified_a_type == unified_b_type {
                    Ok(InferredType::Option(Box::new(unified_a_type)))
                } else {
                    Err("Record fields do not match".to_string())
                }
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
                            return Err("Record fields do not match".to_string());
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
                            return Err("Record fields do not match".to_string());
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
                    return Err("Variant fields do not match".to_string());
                }

                let mut variants = a_variants.clone();

                for (variant, a_type) in variants.iter_mut() {
                    if let Some((_, b_type)) = b_variants
                        .iter()
                        .find(|(b_variant, _)| b_variant == variant)
                    {
                        let unified_b_type = b_type.unify_types()?;
                        let unified_a_type = a_type.unify_types()?;
                        if unified_a_type == unified_b_type {
                            *a_type = unified_a_type
                        } else {
                            return Err("Variant fields do not match".to_string());
                        }
                    } else {
                        return Err("Variant fields do not match".to_string());
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
                    Err("Resource id or mode do not match".to_string())
                }
            }

            (InferredType::AllOf(a_types), inferred_types) => {
                let unified_all_types = Self::unify_all_required_types(a_types)?;
                let alternative_type = inferred_types.unify_types()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err("AllOf types do not match".to_string())
                }
            }

            (inferred_types, InferredType::AllOf(b_types)) => {
                let unified_all_types = Self::unify_all_required_types(b_types)?;
                let alternative_type = inferred_types.unify_types()?;

                if unified_all_types == alternative_type {
                    Ok(unified_all_types)
                } else {
                    Err("AllOf types do not match".to_string())
                }
            }

            // In all other cases, it should match exact
            (a, b) => {
                if a == b {
                    Ok(a.clone())
                } else {
                    Err("Types do not match".to_string())
                }
            }
        }
    }

    // Unify types where both types do matter. Example in reality x can form to be both U64 and U32 in the IR, resulting in AllOf
    // Result of this type hardly becomes OneOf
    fn unify_with_required(&self, other: &InferredType) -> Result<InferredType, Vec<String>> {
        match (self, other) {
            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                let mut fields = HashMap::new();
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        fields.insert(a_name.clone(), a_type.unify_with_required(b_type)?);
                    }
                }
                Ok(InferredType::Record(
                    fields.iter().map(|(n, t)| (n.clone(), t.clone())).collect(),
                ))
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
                    if !one_of_types.contains(&typ) {
                        return Err(vec!["AllOf types are not part of OneOf types".to_string()]);
                    }
                }
                // Once we know the types in the AllOf are part of OneOf, we can simply return the unified all-of
                Self::unify_all_required_types(types)
            }

            // Given we always flatten AllOf and OneOf, in reality we can check if All of the types are part of the One ofs.
            (InferredType::OneOf(one_of_types), InferredType::AllOf(all_of_types)) => {
                for required_type in all_of_types {
                    if !one_of_types.contains(&required_type) {
                        return Err(vec!["OneOf types are not part of AllOf types".to_string()]);
                    }
                }
                Self::unify_all_required_types(all_of_types)
            }

            (InferredType::OneOf(types), inferred_type) => {
                if types.contains(inferred_type) {
                    Ok(inferred_type.clone())
                } else {
                    Err(vec!["OneOf types do not match".to_string()])
                }
            }

            (inferred_type, InferredType::OneOf(types)) => {
                if types.contains(inferred_type) {
                    Ok(inferred_type.clone())
                } else {
                    Err(vec!["OneOf types do not match".to_string()])
                }
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
                    Err(vec!["Types do not match".to_string()])
                }
            }
        }
    }

    pub fn type_check(&self) -> Result<(), Vec<TypeErrorMessage>> {
        let mut errors = Vec::new();

        match self {
            InferredType::AllOf(types) => {
                if !self.check_all_compatible(types) {
                    errors.push(TypeErrorMessage("Incompatible types resolved".to_string()));
                }
            }
            InferredType::OneOf(_) => {
                errors.push(TypeErrorMessage("Failed to resolve types".to_string()));
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
        match self {
            InferredType::S8
            | InferredType::U8
            | InferredType::S16
            | InferredType::U16
            | InferredType::S32
            | InferredType::U32
            | InferredType::S64
            | InferredType::U64
            | InferredType::F32
            | InferredType::F64 => true,
            _ => false,
        }
    }

    fn is_string(&self) -> bool {
        match self {
            InferredType::Str => true,
            _ => false,
        }
    }

    fn check_all_compatible(&self, types: &Vec<InferredType>) -> bool {
        if types.len() > 1 {
            for i in 0..types.len() {
                for j in (i + 1)..types.len() {
                    if !self.are_compatible(&types[i], &types[j]) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn are_compatible(&self, a: &InferredType, b: &InferredType) -> bool {
        match (a, b) {
            (InferredType::List(a_type), InferredType::List(b_type)) => {
                self.are_compatible(a_type, b_type)
            }

            (InferredType::Tuple(a_types), InferredType::Tuple(b_types)) => {
                if a_types.len() != b_types.len() {
                    return false;
                }
                for (a_type, b_type) in a_types.iter().zip(b_types) {
                    if !self.are_compatible(a_type, b_type) {
                        return false;
                    }
                }
                true
            }

            (InferredType::Record(a_fields), InferredType::Record(b_fields)) => {
                for (a_name, a_type) in a_fields {
                    if let Some((_, b_type)) = b_fields.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        if !self.are_compatible(a_type, b_type) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            }

            (InferredType::Flags(a_flags), InferredType::Flags(b_flags)) => {
                a_flags == b_flags
            }

            (InferredType::Enum(a_variants), InferredType::Enum(b_variants)) => {
                a_variants == b_variants
            }

            (InferredType::Option(a_type), InferredType::Option(b_type)) => {
                self.are_compatible(a_type, b_type)
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
                match (a_ok, b_ok) {
                    (Some(a_inner), Some(b_inner)) => self.are_compatible(a_inner, b_inner),
                    (None, None) => true,
                    (Some(_), None) => true,
                    (None, Some(_)) => true,
                }
                match (a_error, b_error) {
                    (Some(a_inner), Some(b_inner)) => self.are_compatible(a_inner, b_inner),
                    (None, None) => true,
                    (Some(_), None) => true,
                    (None, Some(_)) => true,
                }
            }

            (InferredType::Variant(a_variants), InferredType::Variant(b_variants)) => {
                for (a_name, a_type) in a_variants {
                    if let Some((_, b_type)) =
                        b_variants.iter().find(|(b_name, _)| b_name == a_name)
                    {
                        match (a_type, b_type) {
                            (Some(a_inner), Some(b_inner)) => {
                                if !self.are_compatible(a_inner, b_inner) {
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
                    if !self.are_compatible(t, inferred_type) {
                        return false;
                    }
                }
                true
            }

            (inferred_type, InferredType::AllOf(types)) => {
                for t in types {
                    if !self.are_compatible(inferred_type, t) {
                        return false;
                    }
                }
                true
            }

            (InferredType::OneOf(types), inferred_type) => {
                if types.contains(inferred_type) {
                    true
                } else {
                    false
                }
            }

            (inferred_type, InferredType::OneOf(types)) => {
                if types.contains(inferred_type) {
                    true
                } else {
                    false
                }
            }

            (InferredType::Unknown, _) | (_, InferredType::Unknown) => true,

            (a, b) => a.is_number() && b.is_number() || a.is_string() && b.is_string(),

            _ => false,
        }
    }

    // The only to update inferred type is to discard unknown types
    // and push that as `allOf`
    pub fn update(&mut self, new_inferred_type: InferredType) {
        match self {
            InferredType::Unknown => {
                *self = new_inferred_type;
            }
            InferredType::AllOf(types) => match new_inferred_type {
                InferredType::AllOf(new_types) => {
                    types.extend(new_types);
                }
                _ => {
                    types.push(new_inferred_type);
                }
            },
            InferredType::OneOf(types) => match new_inferred_type {
                InferredType::OneOf(new_types) => {
                    types.extend(new_types);
                }
                _ => {
                    types.push(new_inferred_type);
                }
            },

            // Any other types simply indicates it can be all of those types
            // until type checked
            _ => {
                // As far as the new inferred type is not unknown, we add it to all of
                if new_inferred_type != InferredType::Unknown {
                    *self = InferredType::AllOf(vec![self.clone(), new_inferred_type])
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
            AnalysedType::List(t) => InferredType::List(Box::new(t.into())),
            AnalysedType::Tuple(ts) => {
                InferredType::Tuple(ts.into_iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => {
                InferredType::Record(fs.into_iter().map(|(n, t)| (n, t.into())).collect())
            }
            AnalysedType::Flags(vs) => InferredType::Flags(vs),
            AnalysedType::Enum(vs) => InferredType::Enum(vs),
            AnalysedType::Option(t) => InferredType::Option(Box::new(t.into())),
            AnalysedType::Result(TypedResult { ok, error, .. }) => InferredType::Result {
                ok: ok.map(|t| Box::new(t.into())),
                error: error.map(|t| Box::new(t.into())),
            },
            AnalysedType::Variant(vs) => InferredType::Variant(
                vs.into_iter()
                    .map(|(n, t)| (n, t.map(|t| t.into())))
                    .collect(),
            ),
            AnalysedType::Handle(TypedHandle { typ, .. }) => match typ {
                Some(type_handle) => InferredType::Resource {
                    resource_id: type_handle.resource_id,
                    resource_mode: type_handle.mode,
                },
                None => InferredType::Unknown,
            },
        }
    }
}
