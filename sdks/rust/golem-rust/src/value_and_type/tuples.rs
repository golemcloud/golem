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

use crate::value_and_type::type_builder::TypeNodeBuilder;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::NodeBuilder;

// 2-tuple
impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<A: FromValueAndType, B: FromValueAndType> FromValueAndType for (A, B) {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 2-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 2-tuple".to_string())?,
        )?;
        Ok((a, b))
    }
}

// 3-tuple
impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoValue for (A, B, C) {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<A: FromValueAndType, B: FromValueAndType, C: FromValueAndType> FromValueAndType for (A, B, C) {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        Ok((a, b, c))
    }
}

// 4-tuple
impl<A: IntoValue, B: IntoValue, C: IntoValue, D: IntoValue> IntoValue for (A, B, C, D) {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<A: FromValueAndType, B: FromValueAndType, C: FromValueAndType, D: FromValueAndType>
    FromValueAndType for (A, B, C, D)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 4-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 4-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 4-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 4-tuple".to_string())?,
        )?;
        Ok((a, b, c, d))
    }
}

// 5-tuple
impl<A: IntoValue, B: IntoValue, C: IntoValue, D: IntoValue, E: IntoValue> IntoValue
    for (A, B, C, D, E)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 5-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 5-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 5-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 5-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 5-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e))
    }
}

// 6-tuple
impl<A: IntoValue, B: IntoValue, C: IntoValue, D: IntoValue, E: IntoValue, F: IntoValue> IntoValue
    for (A, B, C, D, E, F)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 6-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f))
    }
}

// 7-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 7-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g))
    }
}

// 8-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
        H: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G, H)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder = self.7.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder = H::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
        H: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G, H)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        let h = H::from_extractor(
            &extractor
                .tuple_element(7)
                .ok_or_else(|| "Expected 8-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g, h))
    }
}

// 9-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
        H: IntoValue,
        I: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G, H, I)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder = self.7.add_to_builder(builder.item());
        builder = self.8.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder = H::add_to_type_builder(builder.item());
        builder = I::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
        H: FromValueAndType,
        I: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G, H, I)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let h = H::from_extractor(
            &extractor
                .tuple_element(7)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        let i = I::from_extractor(
            &extractor
                .tuple_element(8)
                .ok_or_else(|| "Expected 9-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g, h, i))
    }
}

// 10-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
        H: IntoValue,
        I: IntoValue,
        J: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G, H, I, J)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder = self.7.add_to_builder(builder.item());
        builder = self.8.add_to_builder(builder.item());
        builder = self.9.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder = H::add_to_type_builder(builder.item());
        builder = I::add_to_type_builder(builder.item());
        builder = J::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
        H: FromValueAndType,
        I: FromValueAndType,
        J: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G, H, I, J)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let h = H::from_extractor(
            &extractor
                .tuple_element(7)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let i = I::from_extractor(
            &extractor
                .tuple_element(8)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        let j = J::from_extractor(
            &extractor
                .tuple_element(9)
                .ok_or_else(|| "Expected 10-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g, h, i, j))
    }
}

// 11-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
        H: IntoValue,
        I: IntoValue,
        J: IntoValue,
        K: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G, H, I, J, K)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder = self.7.add_to_builder(builder.item());
        builder = self.8.add_to_builder(builder.item());
        builder = self.9.add_to_builder(builder.item());
        builder = self.10.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder = H::add_to_type_builder(builder.item());
        builder = I::add_to_type_builder(builder.item());
        builder = J::add_to_type_builder(builder.item());
        builder = K::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
        H: FromValueAndType,
        I: FromValueAndType,
        J: FromValueAndType,
        K: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G, H, I, J, K)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let h = H::from_extractor(
            &extractor
                .tuple_element(7)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let i = I::from_extractor(
            &extractor
                .tuple_element(8)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let j = J::from_extractor(
            &extractor
                .tuple_element(9)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        let k = K::from_extractor(
            &extractor
                .tuple_element(10)
                .ok_or_else(|| "Expected 11-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g, h, i, j, k))
    }
}

// 12-tuple
impl<
        A: IntoValue,
        B: IntoValue,
        C: IntoValue,
        D: IntoValue,
        E: IntoValue,
        F: IntoValue,
        G: IntoValue,
        H: IntoValue,
        I: IntoValue,
        J: IntoValue,
        K: IntoValue,
        L: IntoValue,
    > IntoValue for (A, B, C, D, E, F, G, H, I, J, K, L)
{
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder = self.3.add_to_builder(builder.item());
        builder = self.4.add_to_builder(builder.item());
        builder = self.5.add_to_builder(builder.item());
        builder = self.6.add_to_builder(builder.item());
        builder = self.7.add_to_builder(builder.item());
        builder = self.8.add_to_builder(builder.item());
        builder = self.9.add_to_builder(builder.item());
        builder = self.10.add_to_builder(builder.item());
        builder = self.11.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder = D::add_to_type_builder(builder.item());
        builder = E::add_to_type_builder(builder.item());
        builder = F::add_to_type_builder(builder.item());
        builder = G::add_to_type_builder(builder.item());
        builder = H::add_to_type_builder(builder.item());
        builder = I::add_to_type_builder(builder.item());
        builder = J::add_to_type_builder(builder.item());
        builder = K::add_to_type_builder(builder.item());
        builder = L::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<
        A: FromValueAndType,
        B: FromValueAndType,
        C: FromValueAndType,
        D: FromValueAndType,
        E: FromValueAndType,
        F: FromValueAndType,
        G: FromValueAndType,
        H: FromValueAndType,
        I: FromValueAndType,
        J: FromValueAndType,
        K: FromValueAndType,
        L: FromValueAndType,
    > FromValueAndType for (A, B, C, D, E, F, G, H, I, J, K, L)
{
    fn from_extractor<'a, 'b>(
        extractor: &'a impl golem_wasm::WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let d = D::from_extractor(
            &extractor
                .tuple_element(3)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let e = E::from_extractor(
            &extractor
                .tuple_element(4)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let f = F::from_extractor(
            &extractor
                .tuple_element(5)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let g = G::from_extractor(
            &extractor
                .tuple_element(6)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let h = H::from_extractor(
            &extractor
                .tuple_element(7)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let i = I::from_extractor(
            &extractor
                .tuple_element(8)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let j = J::from_extractor(
            &extractor
                .tuple_element(9)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let k = K::from_extractor(
            &extractor
                .tuple_element(10)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        let l = L::from_extractor(
            &extractor
                .tuple_element(11)
                .ok_or_else(|| "Expected 12-tuple".to_string())?,
        )?;
        Ok((a, b, c, d, e, f, g, h, i, j, k, l))
    }
}

#[cfg(test)]
mod tests {
    use crate::roundtrip_test;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_tuple2_u32_string,
        (u32, String),
        (0u32.., ".*").prop_map(|(a, b)| (a, b))
    );
    roundtrip_test!(
        prop_roundtrip_tuple3_u32_string_bool,
        (u32, String, bool),
        (0u32.., ".*", proptest::bool::ANY).prop_map(|(a, b, c)| (a, b, c))
    );
}
