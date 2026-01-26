mod model;

use std::collections::HashMap;

use golem_rust::agentic::{BaseAgent, MultimodalAdvanced, Multimodal, UnstructuredBinary, UnstructuredText};
use golem_rust::{agent_definition, agent_implementation, description};
use golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime;

use model::*;

#[agent_definition]
#[description("Rust Code First FooAgent")]
trait FooAgent {
    fn new(opt_string: Option<String>) -> Self;

    fn get_id(&self) -> String;

    async fn fun_string(&self, string: String) -> String;

    async fn fun_string_fire_and_forget(&self, string: String);

    async fn fun_string_later(&self, string: String);

    async fn fun_u8(&mut self, number: u8) -> u8;

    async fn fun_i8(&mut self, number: i8) -> i8;

    async fn fun_u16(&mut self, number: u16) -> u16;

    async fn fun_i16(&mut self, number: i16) -> i16;

    async fn fun_i32(&mut self, number: i32) -> i32;

    async fn fun_u32(&mut self, number: u32) -> u32;

    async fn fun_u64(&mut self, number: u64) -> u64;

    async fn fun_i64(&mut self, number: i64) -> i64;

    async fn fun_f32(&mut self, number: f32) -> f32;

    async fn fun_f64(&mut self, number: f64) -> f64;

    async fn fun_char(&self, char: char) -> char;

    async fn fun_boolean(&self, boolean: bool) -> bool;

    async fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives;

    async fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool);

    async fn fun_tuple_complex(
        &mut self,
        tuple: (String, f64, AllPrimitives, bool),
    ) -> (String, f64, AllPrimitives, bool);

    async fn fun_map(
        &mut self,
        map: std::collections::HashMap<String, i32>,
    ) -> std::collections::HashMap<String, i32>;

    async fn fun_collections(&mut self, collections: Collections) -> Collections;

    async fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct;

    async fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct;

    async fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct;

    async fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum;

    async fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum;

    async fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String>;

    async fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String>;

    async fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()>;

    async fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()>;

    async fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum>;

    async fn fun_option(&mut self, option: Option<String>) -> Option<String>;

    async fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct>;

    async fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals;

    async fn fun_multi_modal(&self, input: MultimodalAdvanced<TextImageData>) -> MultimodalAdvanced<TextImageData>;

    async fn fun_multi_modal_basic(&self, input: Multimodal) -> Multimodal;

    async fn fun_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText;

    async fn fun_unstructured_text_lc(&self, input: UnstructuredText<MyLang>) -> UnstructuredText<MyLang>;

    async fn fun_unstructured_binary(&self, input: UnstructuredBinary<MyMimeType>) -> UnstructuredBinary<MyMimeType>;
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

    fn get_id(&self) -> String {
        self.get_agent_id()
    }

    async fn fun_string(&self, string: String) -> String {
        self.client.fun_string(string).await
    }

    async fn fun_string_fire_and_forget(&self, string: String) {
        self.client.trigger_fun_string(string);
    }

    async fn fun_string_later(&self, string: String) {
        self.client.schedule_fun_string(string, Datetime { seconds: 1, nanoseconds: 1 });
    }

    async fn fun_u8(&mut self, number: u8) -> u8 {
        self.client.fun_u8(number).await
    }

    async fn fun_i8(&mut self, number: i8) -> i8 {
        self.client.fun_i8(number).await
    }

    async fn fun_u16(&mut self, number: u16) -> u16 {
        self.client.fun_u16(number).await
    }

    async fn fun_i16(&mut self, number: i16) -> i16 {
        self.client.fun_i16(number).await
    }

    async fn fun_i32(&mut self, number: i32) -> i32 {
        self.client.fun_i32(number).await
    }

    async fn fun_u32(&mut self, number: u32) -> u32 {
        self.client.fun_u32(number).await
    }

    async fn fun_u64(&mut self, number: u64) -> u64 {
        self.client.fun_u64(number).await
    }

    async fn fun_i64(&mut self, number: i64) -> i64 {
        self.client.fun_i64(number).await
    }

    async fn fun_f32(&mut self, number: f32) -> f32 {
        self.client.fun_f32(number).await
    }

    async fn fun_f64(&mut self, number: f64) -> f64 {
        self.client.fun_f64(number).await
    }

    async fn fun_char(&self, char: char) -> char {
        self.client.fun_char(char).await
    }

    async fn fun_boolean(&self, boolean: bool) -> bool {
        self.client.fun_boolean(boolean).await
    }

    async fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives {
        self.client.fun_all_primitives(all_primitives).await
    }

    async fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool) {
        self.client.fun_tuple_simple(tuple).await
    }

    async fn fun_tuple_complex(
        &mut self,
        tuple: (String, f64, AllPrimitives, bool),
    ) -> (String, f64, AllPrimitives, bool) {
        self.client.fun_tuple_complex(tuple).await
    }

    async fn fun_map(
        &mut self,
        map: HashMap<String, i32>,
    ) -> HashMap<String, i32> {
        self.client.fun_map(map).await
    }

    async fn fun_collections(&mut self, collections: Collections) -> Collections {
        self.client.fun_collections(collections).await
    }

    async fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct {
        self.client.fun_struct_simple(simple_struct).await
    }

    async fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct {
        self.client.fun_struct_nested(nested_struct).await
    }

    async fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct {
        self.client.fun_struct_complex(complex_struct).await
    }

    async fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum {
        self.client.fun_simple_enum(simple_enum).await
    }

    async fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum {
        self.client.fun_complex_enum(complex_enum).await
    }

    async fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String> {
        self.client.fun_result(result).await
    }

    async fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String> {
        self.client.fun_result_unit_ok(result).await
    }

    async fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()> {
        self.client.fun_result_unit_err(result).await
    }

    async fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()> {
        self.client.fun_result_unit_both(result).await
    }

    async fn fun_result_complex(
        &mut self,
        result: Result<NestedStruct, ComplexEnum>,
    ) -> Result<NestedStruct, ComplexEnum> {
        self.client.fun_result_complex(result).await
    }

    async fn fun_option(&mut self, option: Option<String>) -> Option<String> {
        self.client.fun_option(option).await
    }

    async fn fun_option_complex(&mut self, option: Option<NestedStruct>) -> Option<NestedStruct> {
        self.client.fun_option_complex(option).await
    }

    async fn fun_enum_with_only_literals(
        &mut self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals {
        self.client
            .fun_enum_with_only_literals(enum_with_only_literals).await
    }

    async fn fun_multi_modal(&self, input: MultimodalAdvanced<TextImageData>) -> MultimodalAdvanced<TextImageData> {
        self.client.fun_multi_modal(input).await
    }

    async fn fun_multi_modal_basic(&self, input: Multimodal) -> Multimodal {
        self.client.fun_multi_modal_basic(input).await
    }

    async fn fun_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText {
        self.client.fun_unstructured_text(input).await
    }

    async fn fun_unstructured_text_lc(&self, input: UnstructuredText<MyLang>) -> UnstructuredText<MyLang> {
        self.client.fun_unstructured_text_lc(input).await
    }

    async fn fun_unstructured_binary(&self, input: UnstructuredBinary<MyMimeType>) -> UnstructuredBinary<MyMimeType> {
        self.client.fun_unstructured_binary(input).await
    }
}

#[agent_definition]
#[description("Rust Code First BarAgent")]
trait BarAgent {
    fn new(opt_string: Option<String>) -> Self;

    fn get_id(&self) -> String;

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

    fn fun_char(&self, char: char) -> char;

    fn fun_boolean(&self, boolean: bool) -> bool;

    fn fun_all_primitives(&mut self, all_primitives: AllPrimitives) -> AllPrimitives;

    fn fun_tuple_simple(&mut self, tuple: (String, f64, bool)) -> (String, f64, bool);

    fn fun_tuple_complex(
        &mut self,
        tuple: (String, f64, AllPrimitives, bool),
    ) -> (String, f64, AllPrimitives, bool);

    fn fun_map(
        &mut self,
        map: HashMap<String, i32>,
    ) -> HashMap<String, i32>;

    fn fun_collections(&mut self, collections: Collections) -> Collections;

    fn fun_struct_simple(&mut self, simple_struct: SimpleStruct) -> SimpleStruct;

    fn fun_struct_nested(&mut self, nested_struct: NestedStruct) -> NestedStruct;

    fn fun_struct_complex(&mut self, complex_struct: ComplexStruct) -> ComplexStruct;

    fn fun_simple_enum(&mut self, simple_enum: SimpleEnum) -> SimpleEnum;

    fn fun_complex_enum(&mut self, complex_enum: ComplexEnum) -> ComplexEnum;

    fn fun_result(&mut self, result: Result<String, String>) -> Result<String, String>;

    fn fun_result_unit_ok(&mut self, result: Result<(), String>) -> Result<(), String>;

    fn fun_result_unit_err(&mut self, result: Result<String, ()>) -> Result<String, ()>;

    fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()>;

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

    fn fun_multi_modal(&self, input: MultimodalAdvanced<TextImageData>) -> MultimodalAdvanced<TextImageData>;

    fn fun_multi_modal_basic(&self, input: Multimodal) -> Multimodal;

    fn fun_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText;

    fn fun_unstructured_text_lc(&self, input: UnstructuredText<MyLang>) -> UnstructuredText<MyLang>;

    fn fun_unstructured_binary(&self, input: UnstructuredBinary<MyMimeType>) -> UnstructuredBinary<MyMimeType>;
}

struct BarAgentImpl {
    _id: String,
}

#[agent_implementation]
impl BarAgent for BarAgentImpl {
    fn new(opt_string: Option<String>) -> Self {
        let _ = SingletonAgentClient::get();

        BarAgentImpl {
            _id: opt_string.unwrap_or_else(|| "default_id".to_string()),
        }
    }

    fn get_id(&self) -> String {
        self.get_agent_id()
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

    fn fun_char(&self, char: char) -> char {
        char
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

    fn fun_tuple_complex(
        &mut self,
        tuple: (String, f64, AllPrimitives, bool),
    ) -> (String, f64, AllPrimitives, bool) {
        tuple
    }

    fn fun_map(
        &mut self,
        map: HashMap<String, i32>,
    ) -> HashMap<String, i32> {
        map
    }

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

    fn fun_result_unit_both(&mut self, result: Result<(), ()>) -> Result<(), ()> {
        result
    }

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

    fn fun_multi_modal(&self, input: MultimodalAdvanced<TextImageData>) -> MultimodalAdvanced<TextImageData> {
        input
    }

    fn fun_multi_modal_basic(&self, input: Multimodal) -> Multimodal {
        input
    }

    fn fun_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText {
        input
    }

    fn fun_unstructured_text_lc(&self, input: UnstructuredText<MyLang>) -> UnstructuredText<MyLang> {
        input
    }

    fn fun_unstructured_binary(&self, input: UnstructuredBinary<MyMimeType>) -> UnstructuredBinary<MyMimeType> {
        input
    }
}

#[agent_definition]
trait SingletonAgent {
    fn new() -> Self;
    fn get_value(&self) -> u32;
}

struct SingletonImpl{}

#[agent_implementation]
impl SingletonAgent for SingletonImpl {
    fn new() -> Self {
        SingletonImpl{}
    }

    fn get_value(&self) -> u32 {
        42
    }
}
