// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub use refined_type::*;
pub use type_extraction::*;

pub(crate) mod precise_types;
mod refined_type;
mod type_extraction;

use crate::type_refinement::precise_types::*;
use crate::{InferredType, TypeInternal};

/// # Example:
///
/// ### Given:
/// `InferredType::AllOf(InferredType::Option(U64), InferredType::AllOf(InferredType::Option(U32), InferredType::Option(Str)))`
///
/// ### Then:
/// `OptionalType::refine(inferred_type)` returns `RefinedType<OptionalType>` giving direct access to a collection of optional types.
/// Precisely, this is, `RefinedTypes::AllOf(FlattenedTypes { required_types: [OptionalType(U64), OptionalType(U32), OptionalType(Str)], alternative_types: [] })`
///
/// At this point, from a type-level, we guarantee its _only_ a collection of optional types.
///
/// ### Extracting inner types
/// More interestingly,  we need to extract the type (inner type) of option, and we can do that by calling `inner_type()` on the `RefinedType<OptionalType>` instance,
/// returning `InferredType::AllOf(U64, U32, Str)`. This helps in phases such as type-push down, where we extract the inner type of parent
/// and push it down to the children.
///
/// We can see this in action for complex types such as `Record`, `Result::Ok`, `Result::Err`, `Sequence`, `Tuple` etc.
/// `TypeRefinement` gives a precise structured solution instead of adhoc loops and control structures floating all over the rib codebase.
///
/// ## Details:
///
/// The `TypeRefinement` trait defines a method for refining `InferredType` instances into more precise types.
/// Refinement involves transforming broad type categories, like `InferredType::AllOf` and `InferredType::OneOf`,
/// that are possibly deeply nested into a clear flattened structure of types. See `RefinedType`,
///
/// The idea is breaking down such complex type groupings into
/// clearer, more manageable forms. For instance, an `InferredType::AllOf` containing optional types can be refined
/// to an `AllOf` with the non-optional, underlying types extracted and organized into `required_types` and
/// `alternative_types` in the `FlattenedTypes` struct.
///
/// This refinement enables better handling of nuanced combinations of types:
///
/// - `InferredType::OneOf(f1, f2, InferredType::AllOf(f3, f4))`
///   could be refined to `RefinedTypes::OneOf(FlattenedTypes { required_types: [f1, f2], alternative_types: [f3, f4] })`
///
/// - `InferredType::AllOf(f1, f2, InferredType::OneOf(f3, f4))`
///   could be refined to `RefinedTypes::AllOf(FlattenedTypes { required_types: [f1, f2], alternative_types: [f3, f4] })`
///
/// By applying these refinements, the `TypeRefinement` trait makes type inference more robust, allowing for
/// precise interpretation and manipulation of complex type structures in various contexts.
///
/// This granularity improves the accuracy and clarity of type handling, particularly when working with intricate
/// combinations and nested structures within type inference logic.
pub trait TypeRefinement {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>>
    where
        Self: Sized;
}

impl TypeRefinement for RecordType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Record(record_type) = inferred_type.internal_type() {
                Some(RecordType(record_type.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for OptionalType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Option(optional_type) = inferred_type.internal_type() {
                Some(OptionalType(optional_type.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for OkType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Result { ok, .. } = inferred_type.internal_type() {
                Some(OkType(ok.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for ErrType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Result { error, .. } = inferred_type.internal_type() {
                Some(ErrType(error.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for ListType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::List(inferred_type) = inferred_type.internal_type() {
                Some(ListType(inferred_type.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for RangeType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>>
    where
        Self: Sized,
    {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Range { from, to } = inferred_type.internal_type() {
                Some(RangeType(from.clone(), to.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for TupleType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Tuple(tuple_type) = inferred_type.internal_type() {
                Some(TupleType(tuple_type.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for StringType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Str = inferred_type.internal_type() {
                Some(StringType)
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for NumberType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| match inferred_type
            .inner
            .as_ref()
        {
            TypeInternal::S8 => Some(NumberType),
            TypeInternal::S16 => Some(NumberType),
            TypeInternal::S32 => Some(NumberType),
            TypeInternal::S64 => Some(NumberType),
            TypeInternal::U8 => Some(NumberType),
            TypeInternal::U16 => Some(NumberType),
            TypeInternal::U32 => Some(NumberType),
            TypeInternal::U64 => Some(NumberType),
            TypeInternal::F32 => Some(NumberType),
            TypeInternal::F64 => Some(NumberType),
            _ => None,
        })
    }
}

impl TypeRefinement for BoolType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Bool = inferred_type.internal_type() {
                Some(BoolType)
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for CharType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Chr = inferred_type.internal_type() {
                Some(CharType)
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for FlagsType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Flags(flags) = inferred_type.internal_type() {
                Some(FlagsType(flags.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for EnumType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Enum(enums) = inferred_type.internal_type() {
                Some(EnumType(enums.clone()))
            } else {
                None
            }
        })
    }
}

impl TypeRefinement for VariantType {
    fn refine(inferred_type: &InferredType) -> Option<RefinedType<Self>> {
        internal::refine_inferred_type(inferred_type, &|inferred_type| {
            if let TypeInternal::Variant(variant_type) = inferred_type.internal_type() {
                Some(VariantType(variant_type.clone()))
            } else {
                None
            }
        })
    }
}

mod internal {
    use crate::type_refinement::RefinedType;
    use crate::{InferredType, TypeInternal};

    pub(crate) fn refine_inferred_type<F, A>(
        inferred_type: &InferredType,
        select: &F,
    ) -> Option<RefinedType<A>>
    where
        F: Fn(&InferredType) -> Option<A>,
    {
        match inferred_type.internal_type() {
            TypeInternal::AllOf(types) => {
                let mut refined_all_of = vec![];

                for typ in types {
                    if let Some(refined) = refine_inferred_type(typ, select) {
                        refined_all_of.push(refined);
                    } else {
                        return None;
                    }
                }

                Some(RefinedType::AllOf(refined_all_of))
            }
            _ => select(inferred_type).map(RefinedType::Value),
        }
    }
}

#[cfg(test)]
mod type_refinement_tests {
    use test_r::test;

    use crate::type_refinement::precise_types::OptionalType;
    use crate::type_refinement::{RefinedType, TypeRefinement};
    use crate::InferredType;

    #[test]
    fn test_type_refinement_option() {
        let inferred_type = InferredType::option(InferredType::u64());

        let refined_type = OptionalType::refine(&inferred_type).unwrap();

        let expected_refine_type = RefinedType::Value(OptionalType(InferredType::u64()));

        let inner_type = refined_type.inner_type();
        let expected_inner_type = InferredType::u64();

        assert_eq!(refined_type, expected_refine_type);
        assert_eq!(inner_type, expected_inner_type);
    }

    #[test]
    fn test_type_refinement_option_all_of() {
        let types = vec![
            InferredType::option(InferredType::u64()),
            InferredType::option(InferredType::u32()),
            InferredType::option(InferredType::string()),
        ];

        let inferred_type = InferredType::all_of(types);

        let refined_type = OptionalType::refine(&inferred_type).unwrap();

        let expected_refine_type = RefinedType::Value(OptionalType(InferredType::all_of(vec![
            InferredType::u64(),
            InferredType::u32(),
            InferredType::string(),
        ])));

        let inner_type = refined_type.inner_type();
        let expected_inner_types = vec![
            InferredType::u64(),
            InferredType::u32(),
            InferredType::string(),
        ];

        let expected_inner_type = InferredType::all_of(expected_inner_types);

        assert_eq!(refined_type, expected_refine_type);
        assert_eq!(inner_type, expected_inner_type);
    }

    #[test]
    fn test_type_refinement_option_nested_all_of() {
        let inferred_type = InferredType::all_of(vec![
            InferredType::option(InferredType::u64()),
            InferredType::all_of(vec![
                InferredType::option(InferredType::u32()),
                InferredType::option(InferredType::string()),
            ]),
        ]);

        let refined_type = OptionalType::refine(&inferred_type).unwrap();

        let expected_refine_type = RefinedType::Value(OptionalType(InferredType::all_of(vec![
            InferredType::u64(),
            InferredType::u32(),
            InferredType::string(),
        ])));

        let inner_type = refined_type.inner_type();
        let expected_inner_type = InferredType::all_of(vec![
            InferredType::u64(),
            InferredType::u32(),
            InferredType::string(),
        ]);

        assert_eq!(refined_type, expected_refine_type);
        assert_eq!(inner_type, expected_inner_type);
    }
}
