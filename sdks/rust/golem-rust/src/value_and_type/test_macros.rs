// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// Macro to reduce boilerplate for roundtrip property tests.
///
/// Tests that a value can be converted to WitValue and back to the original type.
#[macro_export]
macro_rules! roundtrip_test {
    ($name:ident, $type:ty, $strategy:expr) => {
        #[test]
        fn $name() {
            use crate::value_and_type::{FromValueAndType, IntoValue};
            use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
            use proptest::proptest;
            use proptest::prop_assert_eq;

            proptest!(|(value in $strategy)| {
                let typ = <$type>::get_type();
                let value_and_type = ValueAndType {
                    value: value.clone().into_value(),
                    typ,
                };
                let recovered = <$type>::from_value_and_type(value_and_type)
                    .expect("roundtrip conversion should succeed");
                prop_assert_eq!(recovered, value);
            });
        }
    };
}

/// Macro for types that need a constructor/mapper (like Rc::new, NonZeroX::new, etc.)
#[macro_export]
macro_rules! roundtrip_test_map {
    ($name:ident, $type:ty, $strategy:expr, |$val:ident| $map_expr:expr) => {
        #[test]
        fn $name() {
            use crate::value_and_type::{FromValueAndType, IntoValue};
            use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
            use proptest::proptest;
            use proptest::prop_assert_eq;

            proptest!(|($val in $strategy)| {
                let value = $map_expr;
                let typ = <$type>::get_type();
                let value_and_type = ValueAndType {
                    value: value.clone().into_value(),
                    typ,
                };
                let recovered = <$type>::from_value_and_type(value_and_type)
                    .expect("roundtrip conversion should succeed");
                prop_assert_eq!(recovered, value);
            });
        }
    };
}

/// Macro for wrapper types (Rc, Arc) that need dereferencing for comparison
#[macro_export]
macro_rules! roundtrip_test_deref {
    ($name:ident, $type:ty, $strategy:expr, |$val:ident| $map_expr:expr) => {
        #[test]
        fn $name() {
            use crate::value_and_type::{FromValueAndType, IntoValue};
            use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
            use proptest::proptest;
            use proptest::prop_assert_eq;

            proptest!(|($val in $strategy)| {
                let value = $map_expr;
                let typ = <$type>::get_type();
                let value_and_type = ValueAndType {
                    value: value.clone().into_value(),
                    typ,
                };
                let recovered = <$type>::from_value_and_type(value_and_type)
                    .expect("roundtrip conversion should succeed");
                prop_assert_eq!(*recovered, *value);
            });
        }
    };
}
