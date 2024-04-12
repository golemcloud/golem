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

pub fn call_compensation_function<In, Out, Err>(
    f: impl CompensationFunction<In, Out, Err>,
    result: Option<Result<impl TupleOrUnit<Out>, Err>>,
    input: impl TupleOrUnit<In>,
) -> Result<(), Err> {
    f.call(input, result)
}

pub trait TupleOrUnit<T> {
    fn into(self) -> T;
}

pub trait CompensationFunction<In, Out, Err> {
    fn call(
        self,
        input: impl TupleOrUnit<In>,
        result: Option<Result<impl TupleOrUnit<Out>, Err>>,
    ) -> Result<(), Err>;
}

impl<F, Err> CompensationFunction<(), (), (Err,)> for F
where
    F: FnOnce() -> Result<(), Err>,
{
    fn call(
        self,
        _input: impl TupleOrUnit<()>,
        _result: Option<Result<impl TupleOrUnit<()>, (Err,)>>,
    ) -> Result<(), (Err,)> {
        self().map_err(|e| (e,))?;
        Ok(())
    }
}

impl<F, Out, Err> CompensationFunction<(), (Out,), (Err,)> for F
where
    F: FnOnce(Option<Result<Out, Err>>) -> Result<(), Err>,
{
    fn call(
        self,
        _input: impl TupleOrUnit<()>,
        result: Option<Result<impl TupleOrUnit<(Out,)>, (Err,)>>,
    ) -> Result<(), (Err,)> {
        match result {
            Some(Ok(out)) => {
                let (out,) = out.into();
                self(Some(Ok(out))).map_err(|err| (err,))
            }
            Some(Err((err,))) => self(Some(Err(err))).map_err(|err| (err,)),
            None => self(None).map_err(|err| (err,)),
        }
    }
}
//
// impl<F, In, Out, Err> CompensationFunction<(In,), (Out,), (Err,)> for F
// where
//     F: FnOnce(Option<Result<Out, Err>>, In) -> Result<(), Err>,
// {
//     fn call(
//         self,
//         input: impl TupleOrUnit<(In,)>,
//         result: Option<Result<impl TupleOrUnit<(Out,)>, (Err,)>>,
//     ) -> Result<(), (Err,)> {
//         let input = input.into();
//         match result {
//             Some(Ok(out)) => {
//                 let (out,) = out.into();
//                 self(Some(Ok(out)), input.0).map_err(|err| (err,))
//             }
//             Some(Err((err,))) => self(Some(Err(err)), input.0).map_err(|err| (err,)),
//             None => self(None, input.0).map_err(|err| (err,)),
//         }
//     }
// }
//
// impl<F, In1, In2, Out, Err> CompensationFunction<(In1, In2), (Out,), (Err,)> for F
// where
//     F: FnOnce(Option<Result<Out, Err>>, In1, In2) -> Result<(), Err>,
// {
//     fn call(
//         self,
//         input: impl TupleOrUnit<(In1, In2)>,
//         result: Option<Result<impl TupleOrUnit<(Out,)>, (Err,)>>,
//     ) -> Result<(), (Err,)> {
//         let input = input.into();
//         match result {
//             Some(Ok(out)) => {
//                 let (out,) = out.into();
//                 self(Some(Ok(out)), input.0, input.1).map_err(|err| (err,))
//             }
//             Some(Err((err,))) => self(Some(Err(err)), input.0, input.1).map_err(|err| (err,)),
//             None => self(None, input.0, input.1).map_err(|err| (err,)),
//         }
//     }
// }

impl<T> TupleOrUnit<()> for T {
    fn into(self) -> () {
        ()
    }
}

macro_rules! tuple_or_unit {
    ($($ty:ident),*) => {
        impl<$($ty),*> TupleOrUnit<($($ty,)*)> for ($($ty,)*) {
            fn into(self) -> ($($ty,)*) {
                self
            }
        }
    }
}

macro_rules! compensation_function {
    ($($ty:ident),*) => {
        impl<F, $($ty),*, Out, Err> CompensationFunction<($($ty),*,), (Out,), (Err,)> for F
        where
            F: FnOnce(Option<Result<Out, Err>>, $($ty),*) -> Result<(), Err>,
        {
            fn call(
                self,
                input: impl TupleOrUnit<($($ty),*,)>,
                result: Option<Result<impl TupleOrUnit<(Out,)>, (Err,)>>,
            ) -> Result<(), (Err,)> {
                #[allow(non_snake_case)]
                let ( $($ty,)+ ) = input.into();
                match result {
                    Some(Ok(out)) => {
                        let (out,) = out.into();
                        self(Some(Ok(out)), $($ty),*).map_err(|err| (err,))
                    }
                    Some(Err((err,))) => self(Some(Err(err)), $($ty),*).map_err(|err| (err,)),
                    None => self(None, $($ty),*).map_err(|err| (err,)),
                }
            }
        }
    }
}

macro_rules! generate_for_tuples {
    ($name:ident) => {
        $name!(T1);
        $name!(T1, T2);
        $name!(T1, T2, T3);
        $name!(T1, T2, T3, T4);
        $name!(T1, T2, T3, T4, T5);
        $name!(T1, T2, T3, T4, T5, T6);
        $name!(T1, T2, T3, T4, T5, T6, T7);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15);
        $name!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16);
    };
}

generate_for_tuples!(tuple_or_unit);
generate_for_tuples!(compensation_function);
