// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

// The declaration supplies variant membership. Each adapter supplies explicit
// field expressions, or an override for a shape that is not a routine record.
// Record patterns have no `..`: every model field is mapped or explicitly ignored.
macro_rules! entry_pattern {
    (public $case:ident; record $params:ident => $wit:ident { $($field:ident: $value:expr),* $(,)? }) => {
        PublicOplogEntry::$case($params { $($field),* })
    };
    (raw $case:ident; record ($($ignored:ident),*) => $wit:ident { $($field:ident: $value:expr),* $(,)? }) => {
        golem_common::model::oplog::OplogEntry::$case { $($field),*, $($ignored: _),* }
    };
    (from $case:ident; record ($params:ident) { $($field:ident: $value:expr),* $(,)? }) => {
        oplog::OplogEntry::$case($params)
    };
    ($direction:ident $case:ident; override ($pattern:pat) => $value:expr) => { $pattern };
}

macro_rules! entry_value {
    (public $case:ident; record $params:ident => $wit:ident { $($field:ident: $value:expr),* $(,)? }) => {
        Ok(Self::$case(oplog::$wit { $($field: $value),* }))
    };
    (raw $case:ident; record ($($ignored:ident),*) => $wit:ident { $($field:ident: $value:expr),* $(,)? }) => {
        Ok(Self::$case(oplog::$wit { $($field: $value),* }))
    };
    (from $case:ident; record ($params:ident) { $($field:ident: $value:expr),* $(,)? }) => {
        Ok(Self::$case { $($field: $value),* })
    };
    ($direction:ident $case:ident; override ($pattern:pat) => $value:expr) => { $value };
}

macro_rules! generate_entry_conversion {
    (
        [$($case:ident { $($declaration:tt)* })*]
        from ($source:ty => $target:ty)
        before ($input:ident) $before:block
        { $($mapped:ident { $($mapping:tt)* })* }
    ) => {
        $(const _: () = {
            macro_rules! same_variant { ($case) => {}; }
            same_variant!($mapped);
        };)*
        generate_entry_conversion!(@from ($source => $target) ($input) [];
            $($case { $($mapping)* })*
        );
    };
    (@from ($source:ty => $target:ty) ($input:ident) [$($arms:tt)*];) => {
        impl TryFrom<$source> for $target {
            type Error = String;

            fn try_from($input: $source) -> Result<Self, String> {
                match $input { $($arms)* }
            }
        }
    };
    (@from ($source:ty => $target:ty) ($input:ident) [$($arms:tt)*];
        $case:ident { absent } $($rest:tt)*) => {
        generate_entry_conversion!(@from ($source => $target) ($input) [$($arms)*]; $($rest)*);
    };
    (@from ($source:ty => $target:ty) ($input:ident) [$($arms:tt)*];
        $case:ident { $($mapping:tt)* } $($rest:tt)*) => {
        generate_entry_conversion!(@from ($source => $target) ($input) [
            $($arms)*
            entry_pattern!(from $case; $($mapping)*) => entry_value!(from $case; $($mapping)*),
        ]; $($rest)*);
    };
    (
        [$($case:ident { $($declaration:tt)* })*]
        $direction:ident ($source:ty => $target:ty)
        before ($input:ident) { $($before:tt)* }
        { $($mapped:ident { $($mapping:tt)* })* }
    ) => {
        // A missing, extra or reordered mapping fails at expansion time rather
        // than silently pairing the wrong variant with a conversion.
        $(const _: () = {
            macro_rules! same_variant { ($case) => {}; }
            same_variant!($mapped);
        };)*

        impl TryFrom<$source> for $target {
            type Error = String;

            fn try_from($input: $source) -> Result<Self, String> {
                $($before)*
                match $input {
                    $(entry_pattern!($direction $case; $($mapping)*) =>
                        entry_value!($direction $case; $($mapping)*),)*
                }
            }
        }
    };
}
