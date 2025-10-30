use golem_rust::{agent_definition, agent_implementation};
use model::*;

mod model;

// #[agent_definition]
// trait FooAgent {
//     fn new(opt_string: Option<String>) -> Self;
//
//     fn fun_all(
//         &self,
//         struct_complex: StructComplexType,
//         union_complex: EnumComplexType,
//         f64: f64,
//         f32: f32,
//         u32: u32,
//         i32: i32,
//         ui6: u16,
//         i16: i16,
//         i8: i8,
//         u8: u8,
//         string: String,
//         bool: bool,
//         map: MapType,
//         tuple_complex: TupleComplexType,
//         tuple: TupleType,
//         list_complex: ListOfObjectType,
//         list_string: ListOfStringType,
//         enum_with_only_literals: EnumWithOnlyLiterals,
//         simple_enum: SimpleEnumType,
//         complex_enum: ComplexEnumType,
//         simple_struct: SimpleStructType,
//         struct_with_single_field: StructWithSingleField,
//         optional_string: OptionalStringType,
//     ) -> String;
//
//     fn fun_optional(
//         &self,
//         param1: Option<String>,
//         param2: Option<StructComplexType>,
//     ) -> Option<String>;
//
//     fn fun_no_return(&self, text: String);
//
//     fn fun_number(&self, number: f64) -> f64;
//
//     fn fun_string(&self, string: String) -> String;
//
//     fn fun_boolean(&self, boolean: bool) -> bool;
//
//     fn fun_map(&self, map: MapType) -> MapType;
//
//     fn fun_struct_complex(&self, complex: StructComplexType) -> StructComplexType;
//
//     fn fun_tuple_complex(&self, complex: TupleComplexType) -> TupleComplexType;
//
//     fn fun_tuple(&self, tuple: TupleType) -> TupleType;
//
//     fn fun_list_complex(&self, list_complex: ListOfObjectType) -> ListOfObjectType;
//
//     fn fun_list_string(&self, list_string: ListOfStringType) -> ListOfStringType;
//
//     fn fun_enum_with_only_literals(
//         &self,
//         enum_with_only_literals: EnumWithOnlyLiterals,
//     ) -> EnumWithOnlyLiterals;
//
//     fn fun_simple_enum(&self, simple_enum: SimpleEnumType) -> SimpleEnumType;
//
//     fn fun_complex_enum(&self, complex_enum: ComplexEnumType) -> ComplexEnumType;
//
//     fn fun_simple_struct(&self, simple_struct: SimpleStructType) -> SimpleStructType;
//
//     fn fun_result(&self, result: Result<String, String>) -> Result<String, String>;
//     // TODO: Add multimodal, unstructured types test cases
// }

// struct FooAgentImpl {
//     bar_agent: Box<dyn BarAgent>,
// }

// #[agent_implementation]
// impl FooAgent for FooAgentImpl {
//     fn new(opt_string: Option<String>) -> Self {
//         let bar_agent = BarAgent::get(opt_string.unwrap_or_else(|| "default_id".to_string())).await;
//
//         FooAgentImpl {
//             bar_agent: Box::new(bar_agent),
//         }
//     }
//
//     fn fun_all(
//         &self,
//         struct_complex: StructComplexType,
//         union_complex: EnumComplexType,
//         f64: f64,
//         f32: f32,
//         u32: u32,
//         i32: i32,
//         ui6: u16,
//         i16: i16,
//         i8: i8,
//         u8: u8,
//         string: String,
//         bool: bool,
//         map: MapType,
//         tuple_complex: TupleComplexType,
//         tuple: TupleType,
//         list_complex: ListOfObjectType,
//         list_string: ListOfStringType,
//         enum_with_only_literals: EnumWithOnlyLiterals,
//         simple_enum: SimpleEnumType,
//         complex_enum: ComplexEnumType,
//         simple_struct: SimpleStructType,
//         struct_with_single_field: StructWithSingleField,
//         optional_string: OptionalStringType,
//     ) -> String {
//         self.bar_agent.fun_all(
//             struct_complex,
//             union_complex,
//             f64,
//             f32,
//             u32,
//             i32,
//             ui6,
//             i16,
//             i8,
//             u8,
//             string,
//             bool,
//             map,
//             tuple_complex,
//             tuple,
//             list_complex,
//             list_string,
//             enum_with_only_literals,
//             simple_enum,
//             complex_enum,
//             simple_struct,
//             struct_with_single_field,
//             optional_string,
//         )
//     }
//
//     fn fun_optional(
//         &self,
//         param1: Option<String>,
//         param2: Option<StructComplexType>,
//     ) -> Option<String> {
//         self.bar_agent.fun_optional(param1, param2)
//     }
//
//     fn fun_no_return(&self, text: String) {
//         self.bar_agent.fun_no_return(text);
//     }
//
//     fn fun_number(&self, number: f64) -> f64 {
//         self.bar_agent.fun_number(number)
//     }
//
//     fn fun_string(&self, string: String) -> String {
//         self.bar_agent.fun_string(string)
//     }
//
//     fn fun_boolean(&self, boolean: bool) -> bool {
//         self.bar_agent.fun_boolean(boolean)
//     }
//
//     fn fun_map(&self, map: MapType) -> MapType {
//         self.bar_agent.fun_map(map)
//     }
//
//     fn fun_struct_complex(&self, complex: StructComplexType) -> StructComplexType {
//         self.bar_agent.fun_struct_complex(complex)
//     }
//
//     fn fun_tuple_complex(&self, complex: TupleComplexType) -> TupleComplexType {
//         self.bar_agent.fun_tuple_complex(complex)
//     }
//
//     fn fun_tuple(&self, tuple: TupleType) -> TupleType {
//         self.bar_agent.fun_tuple(tuple)
//     }
//
//     fn fun_list_complex(&self, list_complex: ListOfObjectType) -> ListOfObjectType {
//         self.bar_agent.fun_list_complex(list_complex)
//     }
//
//     fn fun_list_string(&self, list_string: ListOfStringType) -> ListOfStringType {
//         self.bar_agent.fun_list_string(list_string)
//     }
//
//     fn fun_enum_with_only_literals(
//         &self,
//         enum_with_only_literals: EnumWithOnlyLiterals,
//     ) -> EnumWithOnlyLiterals {
//         self.bar_agent.fun_enum_with_only_literals(enum_with_only_literals)
//     }
//
//     fn fun_simple_enum(&self, simple_enum: SimpleEnumType) -> SimpleEnumType {
//         self.bar_agent.fun_simple_enum(simple_enum)
//     }
//
//     fn fun_complex_enum(&self, complex_enum: ComplexEnumType) -> ComplexEnumType {
//         self.bar_agent.fun_complex_enum(complex_enum)
//     }
//
//     fn fun_simple_struct(&self, simple_struct: SimpleStructType) -> SimpleStructType {
//         self.bar_agent.fun_simple_struct(simple_struct)
//     }
//
//     fn fun_result(&self, result: Result<String, String>) -> Result<String, String> {
//         self.bar_agent.fun_result(result)
//     }
// }

#[agent_definition]
trait BarAgent {
    fn new(opt_string: Option<String>) -> Self;

    // fn fun_all(
    //     &self,
    //     struct_complex: StructComplexType,
    //     union_complex: EnumComplexType,
    //     f64: f64,
    //     f32: f32,
    //     u32: u32,
    //     i32: i32,
    //     ui6: u16,
    //     i16: i16,
    //     i8: i8,
    //     u8: u8,
    //     string: String,
    //     bool: bool,
    //     map: MapType,
    //     tuple_complex: TupleComplexType,
    //     tuple: TupleType,
    //     list_complex: ListOfObjectType,
    //     list_string: ListOfStringType,
    //     enum_with_only_literals: EnumWithOnlyLiterals,
    //     simple_enum: SimpleEnumType,
    //     complex_enum: ComplexEnumType,
    //     simple_struct: SimpleStructType,
    //     struct_with_single_field: StructWithSingleField,
    //     optional_string: OptionalStringType,
    // ) -> String;

    // fn fun_optional(
    //     &self,
    //     param1: Option<String>,
    //     param2: Option<StructComplexType>,
    // ) -> Option<String>;
    //
    // fn fun_no_return(&self, text: String);
    //
    // fn fun_number(&self, number: f64) -> f64;

    fn fun_string(&self, string: String) -> String;

    fn fun_mut(&mut self, string: String) -> String;

    // fn fun_boolean(&self, boolean: bool) -> bool;
    //
    // fn fun_map(&self, map: MapType) -> MapType;
    //
    // fn fun_struct_complex(&self, complex: StructComplexType) -> StructComplexType;
    //
    // fn fun_tuple_complex(&self, complex: TupleComplexType) -> TupleComplexType;
    //
    // fn fun_tuple(&self, tuple: TupleType) -> TupleType;
    //
    // fn fun_list_complex(&self, list_complex: ListOfObjectType) -> ListOfObjectType;
    //
    // fn fun_list_string(&self, list_string: ListOfStringType) -> ListOfStringType;
    //
    // fn fun_enum_with_only_literals(
    //     &self,
    //     enum_with_only_literals: EnumWithOnlyLiterals,
    // ) -> EnumWithOnlyLiterals;
    //
    // fn fun_simple_enum(&self, simple_enum: SimpleEnumType) -> SimpleEnumType;
    //
    // fn fun_complex_enum(&self, complex_enum: ComplexEnumType) -> ComplexEnumType;
    //
    // fn fun_simple_struct(&self, simple_struct: SimpleStructType) -> SimpleStructType;
    //
    // fn fun_result(&self, result: Result<String, String>) -> Result<String, String>;
}

struct BarAgentImpl {
    id: String,
}

#[agent_implementation]
impl BarAgent for BarAgentImpl {
    fn new(opt_string: Option<String>) -> Self {
        BarAgentImpl {
            id: opt_string.unwrap_or_else(|| "default_id".to_string()),
        }
    }

    // fn fun_all(
    //     &self,
    //     _struct_complex: StructComplexType,
    //     _union_complex: EnumComplexType,
    //     _f64: f64,
    //     _f32: f32,
    //     _u32: u32,
    //     _i32: i32,
    //     _ui6: u16,
    //     _i16: i16,
    //     _i8: i8,
    //     _u8: u8,
    //     _string: String,
    //     _bool: bool,
    //     _map: MapType,
    //     _tuple_complex: TupleComplexType,
    //     _tuple: TupleType,
    //     _list_complex: ListOfObjectType,
    //     _list_string: ListOfStringType,
    //     _enum_with_only_literals: EnumWithOnlyLiterals,
    //     _simple_enum: SimpleEnumType,
    //     _complex_enum: ComplexEnumType,
    //     _simple_struct: SimpleStructType,
    //     _struct_with_single_field: StructWithSingleField,
    //     _optional_string: OptionalStringType,
    // ) -> String {
    //     "success".to_string()
    // }
    //
    // fn fun_optional(
    //     &self,
    //     param1: Option<String>,
    //     _param2: Option<StructComplexType>,
    // ) -> Option<String> {
    //     param1
    // }
    //
    // fn fun_no_return(&self, text: String) {
    //     println!("Hello, {}", text);
    // }
    //
    // fn fun_number(&self, number: f64) -> f64 {
    //     number
    // }

    fn fun_string(&self, string: String) -> String {
        string
    }

    fn fun_mut(&mut self, string: String) -> String {
        string
    }

    // fn fun_boolean(&self, boolean: bool) -> bool {
    //     boolean
    // }
    //
    // fn fun_map(&self, map: MapType) -> MapType {
    //     map
    // }
    //
    // fn fun_struct_complex(&self, complex: StructComplexType) -> StructComplexType {
    //     complex
    // }
    //
    // fn fun_tuple_complex(&self, complex: TupleComplexType) -> TupleComplexType {
    //     complex
    // }
    //
    // fn fun_tuple(&self, tuple: TupleType) -> TupleType {
    //     tuple
    // }
    //
    // fn fun_list_complex(&self, list_complex: ListOfObjectType) -> ListOfObjectType {
    //     list_complex
    // }
    //
    // fn fun_list_string(&self, list_string: ListOfStringType) -> ListOfStringType {
    //     list_string
    // }
    //
    // fn fun_enum_with_only_literals(
    //     &self,
    //     enum_with_only_literals: EnumWithOnlyLiterals,
    // ) -> EnumWithOnlyLiterals {
    //     enum_with_only_literals
    // }
    //
    // fn fun_simple_enum(&self, simple_enum: SimpleEnumType) -> SimpleEnumType {
    //     simple_enum
    // }
    //
    // fn fun_complex_enum(&self, complex_enum: ComplexEnumType) -> ComplexEnumType {
    //     complex_enum
    // }
    //
    // fn fun_simple_struct(&self, simple_struct: SimpleStructType) -> SimpleStructType {
    //     simple_struct
    // }
    //
    // fn fun_result(&self, result: Result<String, String>) -> Result<String, String> {
    //     result
    // }
}
