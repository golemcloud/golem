use crate::{InferredType, TypeInternal, TypeOrigin};
use std::collections::HashSet;

// Convert AllOf(AllOf(x, y, z), AllOf(a, b, OneOf(c, d))) to AllOf(x, y, z, a, b, OneOf(c,d))
// In Rib inference, there is no situation of a OneOf having AllOf
// We intentionally make sure we have only AllOf(OneOf) and not OneOf(AllOf)
pub fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut one_of_types = vec![];
    let mut all_of_types = vec![];
    let mut seen = HashSet::new();

    for typ in types {
        match typ.inner.as_ref() {
            TypeInternal::OneOf(types) => {
                let flattened = flatten_one_of_list(types);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        one_of_types.push(t);
                    }
                }
            }
            TypeInternal::AllOf(all_of) => {
                let flattened = flatten_all_of_list(all_of);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        all_of_types.push(t);
                    }
                }
            }
            _ => {
                all_of_types.push(typ.clone());
            }
        }
    }

    if !one_of_types.is_empty() {
        let origins: Vec<TypeOrigin> = one_of_types
            .iter()
            .map(|t| t.origin.clone())
            .collect::<Vec<_>>();
        all_of_types.extend(vec![InferredType::new(
            TypeInternal::OneOf(one_of_types),
            TypeOrigin::Multiple(origins),
        )]);
    }

    all_of_types
}

// Convert OneOf(OneOf(x, y, z), OneOf(a, b)) to OneOf(x, y, z)
// Note that we don't have the situation of OneOf(AllOf) in Rib inference.
// The simplest form of resolving a OneOf is adding information of AllOf in the outer layer.
// Otherwise, `OneOf` is unresolved forever.
pub fn flatten_one_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut one_of_types = Vec::new();
    let mut all_of_types = Vec::new();
    let mut seen = HashSet::new();

    for typ in types {
        match typ.inner.as_ref() {
            TypeInternal::OneOf(types) => {
                let flattened = flatten_one_of_list(types);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        one_of_types.push(t);
                    }
                }
            }
            TypeInternal::AllOf(types) => {
                let flattened = flatten_all_of_list(types);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        all_of_types.push(t);
                    }
                }
            }
            _ => {
                if seen.insert(typ.clone()) {
                    one_of_types.push(typ.clone());
                }
            }
        }
    }

    if !all_of_types.is_empty() {
        let origins: Vec<TypeOrigin> = all_of_types
            .iter()
            .map(|t| t.origin.clone())
            .collect::<Vec<_>>();

        one_of_types.extend(vec![InferredType::new(
            TypeInternal::AllOf(all_of_types),
            TypeOrigin::Multiple(origins),
        )]);
    }

    one_of_types
}
