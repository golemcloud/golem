mod model;

use golem_rust::agentic::Agent;
use golem_rust::{agent_definition, agent_implementation};

use model::*;

#[agent_definition]
trait FooAgent {
    fn new(opt_string: Option<String>) -> Self;

    fn get_agent_id(&self) -> String;

    fn fun_string(&self, string: String) -> String;

    fn fun_u8(&mut self, number: u8) -> u8;

    fn fun_i8(&mut self, number: i8) -> i8;

    fn fun_u16(&mut self, number: u16) -> u16;

    fn fun_i16(&mut self, number: i16) -> i16;

    fn fun_i32(&mut self, number: i32) -> i32;

    fn fun_u32(&mut self, number: u32) -> u32;

    fn fun_u64(&mut self, number: u64) -> u64;

    fn fun_i64(&mut self, number: i64) -> i64;

    fn fun_f32(&mut self, number: f32) -> f32;

    fn fun_f64(&mut self, number: f64) -> f64;

    fn fun_boolean(&self, boolean: bool) -> bool;

    fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives;

    fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool);

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2276
    // fn fun_tuple_complex(
    //     &mut self,
    //     tuple: (String, f64, AllPrimitives, bool),
    // ) -> (String, f64, AllPrimitives, bool);
    //
    fn fun_collections(&mut self, collections: Collections) -> Collections;

    fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct;

    fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct;

    fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct;

    fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum;

    fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum;

    fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String>;

    fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String>;

    fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()>;

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2279
    // fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()>;

    fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum>;

    fn fun_option(&mut self, option: Option<String>) -> Option<String>;

    fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct>;

    fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals;
}

struct FooAgentImpl {
    client: BarAgentClient,
}

#[agent_implementation]
impl FooAgent for FooAgentImpl {
    fn new(opt_string: Option<String>) -> Self {
        FooAgentImpl {
            client: BarAgentClient::get(opt_string),
        }
    }

    fn get_agent_id(&self) -> String {
        self.get_id()
    }

    fn fun_string(&self, string: String) -> String {
        self.client.fun_string(string)
    }

    fn fun_u8(&mut self, number: u8) -> u8 {
        self.client.fun_u8(number)
    }

    fn fun_i8(&mut self, number: i8) -> i8 {
        self.client.fun_i8(number)
    }

    fn fun_u16(&mut self, number: u16) -> u16 {
        self.client.fun_u16(number)
    }

    fn fun_i16(&mut self, number: i16) -> i16 {
        self.client.fun_i16(number)
    }

    fn fun_i32(&mut self, number: i32) -> i32 {
        self.client.fun_i32(number)
    }

    fn fun_u32(&mut self, number: u32) -> u32 {
        self.client.fun_u32(number)
    }

    fn fun_u64(&mut self, number: u64) -> u64 {
        self.client.fun_u64(number)
    }

    fn fun_i64(&mut self, number: i64) -> i64 {
        self.client.fun_i64(number)
    }

    fn fun_f32(&mut self, number: f32) -> f32 {
        self.client.fun_f32(number)
    }

    fn fun_f64(&mut self, number: f64) -> f64 {
        self.client.fun_f64(number)
    }

    fn fun_boolean(&self, boolean: bool) -> bool {
        self.client.fun_boolean(boolean)
    }

    fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives {
        self.client.fun_all_primitives(all_primitives)
    }

    fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool) {
        self.client.fun_tuple_simple(tuple)
    }

    fn fun_collections(&mut self, collections: Collections) -> Collections {
        self.client.fun_collections(collections)
    }

    fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct {
        self.client.fun_struct_simple(simple_struct)
    }

    fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct {
        self.client.fun_struct_nested(nested_struct)
    }

    fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct {
        self.client.fun_struct_complex(complex_struct)
    }

    fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum {
        self.client.fun_simple_enum(simple_enum)
    }

    fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum {
        self.client.fun_complex_enum(complex_enum)
    }

    fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String> {
        self.client.fun_result(result)
    }

    fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String> {
        self.client.fun_result_unit_ok(result)
    }

    fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()> {
        self.client.fun_result_unit_err(result)
    }

    fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum> {
        self.client.fun_result_complex(result)
    }

    fn fun_option(&mut self, option: Option<String>) -> Option<String> {
        self.client.fun_option(option)
    }

    fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct> {
        self.client.fun_option_complex(option)
    }

    fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals {
        self.client
            .fun_enum_with_only_literals(enum_with_only_literals)
    }
}

#[agent_definition]
trait BarAgent {
    fn new(opt_string: Option<String>) -> Self;

    fn get_agent_id(&self) -> String;

    fn fun_string(&self, string: String) -> String;

    fn fun_u8(&mut self, number: u8) -> u8;

    fn fun_i8(&mut self, number: i8) -> i8;

    fn fun_u16(&mut self, number: u16) -> u16;

    fn fun_i16(&mut self, number: i16) -> i16;

    fn fun_i32(&mut self, number: i32) -> i32;

    fn fun_u32(&mut self, number: u32) -> u32;

    fn fun_u64(&mut self, number: u64) -> u64;

    fn fun_i64(&mut self, number: i64) -> i64;

    fn fun_f32(&mut self, number: f32) -> f32;

    fn fun_f64(&mut self, number: f64) -> f64;

    fn fun_boolean(&self, boolean: bool) -> bool;

    fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives;

    fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool);

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2276
    // fn fun_tuple_complex(
    //     &mut self,
    //     tuple: (String, f64, AllPrimitives, bool),
    // ) -> (String, f64, AllPrimitives, bool);
    //
    fn fun_collections(&mut self, collections: Collections) -> Collections;

    fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct;

    fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct;

    fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct;

    fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum;

    fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum;

    fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String>;

    fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String>;

    fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()>;

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2279
    // fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()>;

    fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum>;

    fn fun_option(&mut self, option: Option<String>) -> Option<String>;

    fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct>;

    fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals;
}

struct BarAgentImpl {
    _id: String,
}

#[agent_implementation]
impl BarAgent for BarAgentImpl {
    fn new(opt_string: Option<String>) -> Self {
        BarAgentImpl {
            _id: opt_string.unwrap_or_else(|| "default_id".to_string()),
        }
    }

    fn get_agent_id(&self) -> String {
        self.get_id()
    }

    fn fun_string(&self, string: String) -> String {
        string
    }

    fn fun_u8(&mut self, number: u8) -> u8 {
        number
    }

    fn fun_i8(&mut self, number: i8) -> i8 {
        number
    }

    fn fun_u16(&mut self, number: u16) -> u16 {
        number
    }

    fn fun_i16(&mut self, number: i16) -> i16 {
        number
    }

    fn fun_i32(&mut self, number: i32) -> i32 {
        number
    }

    fn fun_u32(&mut self, number: u32) -> u32 {
        number
    }

    fn fun_u64(&mut self, number: u64) -> u64 {
        number
    }

    fn fun_i64(&mut self, number: i64) -> i64 {
        number
    }

    fn fun_f32(&mut self, number: f32) -> f32 {
        number
    }

    fn fun_f64(&mut self, number: f64) -> f64 {
        number
    }

    fn fun_boolean(&self, boolean: bool) -> bool {
        boolean
    }

    fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives {
        all_primitives
    }

    fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool) {
        tuple
    }

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2276
    // fn fun_tuple_complex(
    //     &mut self,
    //     tuple: (String, f64, AllPrimitives, bool),
    // ) -> (String, f64, AllPrimitives, bool) {
    //     tuple
    // }

    fn fun_collections(&mut self, collections: Collections) -> Collections {
        collections
    }

    fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct {
        simple_struct
    }

    fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct {
        nested_struct
    }

    fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct {
        complex_struct
    }

    fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum {
        simple_enum
    }

    fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum {
        complex_enum
    }

    fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String> {
        result
    }

    fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String> {
        result
    }

    fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()> {
        result
    }

    // TODO; Uncomment after fixing https://github.com/golemcloud/golem/issues/2279
    // fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()> {
    //     result
    // }

    fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum> {
        result
    }

    fn fun_option(&mut self, option: Option<String>) -> Option<String> {
        option
    }

    fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct> {
        option
    }

    fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals {
        enum_with_only_literals
    }
}
