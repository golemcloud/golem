// Copyright 2024 Golem Cloud
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

use crate::InferredType;

// Standalone precise types
#[derive(Clone, PartialEq, Debug)]
pub struct RecordType(pub Vec<(String, InferredType)>);

#[derive(Clone, PartialEq, Debug)]
pub struct OptionalType(pub InferredType);

#[derive(Clone, PartialEq, Debug)]
pub struct OkType(pub InferredType);

#[derive(Clone, PartialEq, Debug)]
pub struct ErrType(pub InferredType);

#[derive(Clone, PartialEq, Debug)]
pub struct ListType(pub InferredType);

#[derive(Clone, PartialEq, Debug)]
pub struct TupleType(pub Vec<InferredType>);

#[derive(Clone, PartialEq, Debug)]
pub struct VariantType(pub Vec<(String, Option<InferredType>)>);
#[derive(Clone, PartialEq, Debug)]
pub struct StringType;

#[derive(Clone, PartialEq, Debug)]
pub struct NumberType;

#[derive(Clone, PartialEq, Debug)]
pub struct CharType;
#[derive(Clone, PartialEq, Debug)]
pub struct BoolType;

#[derive(Clone, PartialEq, Debug)]
pub struct FlagsType(pub Vec<String>);
#[derive(Clone, PartialEq, Debug)]
pub struct EnumType(pub Vec<String>);
