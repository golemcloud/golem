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


pub trait ArenaMember<Arena> {
    type NonRecursive; // The 'Index' version (e.g., usize, Vec<usize>, or the type itself)

    fn deflate(&self, arena: &mut Arena) -> Self::NonRecursive;
    fn inflate(non_rec: Self::NonRecursive, arena: &Arena) -> Self;
}

impl<A, T: ArenaMember<A>> ArenaMember<A> for Box<T> {
    type NonRecursive = T::NonRecursive;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        (**self).deflate(arena)
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        Box::new(T::inflate(val, arena))
    }
}

impl<A, T: ArenaMember<A>> ArenaMember<A> for Option<T> {
    type NonRecursive = Option<T::NonRecursive>;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        self.as_ref().map(|inner| inner.deflate(arena))
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        val.map(|inner| T::inflate(inner, arena))
    }
}


impl<A, T: ArenaMember<A>> ArenaMember<A> for Vec<T> {
    type NonRecursive = Vec<T::NonRecursive>;
    fn deflate(&self, arena: &mut A) -> Self::NonRecursive {
        self.iter().map(|inner| inner.deflate(arena)).collect()
    }
    fn inflate(val: Self::NonRecursive, arena: &A) -> Self {
        val.into_iter().map(|inner| T::inflate(inner, arena)).collect()
    }
}

macro_rules! impl_leaf {
    ($t:ty) => {
        impl<A> ArenaMember<A> for $t {
            type NonRecursive = $t;
            fn deflate(&self, _: &mut A) -> Self::NonRecursive { self.clone() }
            fn inflate(val: Self::NonRecursive, _: &A) -> Self { val }
        }
    };
}

impl_leaf!(i32);
impl_leaf!(String);
impl_leaf!(bool);