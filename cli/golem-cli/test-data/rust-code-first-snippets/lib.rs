use golem_rust::{agent_definition, agent_implementation};
use model::*;

mod model;

#[agent_definition]
trait FooAgent {
    async fn new(opt_string: Option<String>) -> Self;

    fn fun_all(
        &self,
        struct_complex_type: StructComplexType,
        union_complex_type: EnumComplexType,
        f64_type: f64,
        f32_type: f32,
        u32_type: u32,
        i32_type: i32,
        ui6_type: u16,
        i16_type: i16,
        i8_type: i8,
        u8_type: u8,
        string_type: String,
        bool_type: bool,
        map_type: MapType,
        tuple_complex_type: TupleComplexType,
        tuple_type: TupleType,
        list_complex_type: ListOfObjectType,
        list_string_type: ListOfStringType,
        enum_with_only_literals: EnumWithOnlyLiterals,
        simple_enum_type: SimpleEnumType,
        complex_enum_type: ComplexEnumType,
        simple_struct_type: SimpleStructType,
        struct_with_single_field: StructWithSingleField,
        optional_string_type: OptionalStringType,
    ) -> String;

    fn fun_optional(
        &self,
        param1: Option<String>,
        param2: Option<StructComplexType>,
    ) -> Option<String>;

    fn fun_no_return(&self, text: String);

    fn fun_number(&self, number_type: f64) -> f64;

    fn fun_string(&self, string_type: String) -> String;

    fn fun_boolean(&self, boolean_type: bool) -> bool;

    fn fun_map(&self, map_type: MapType) -> MapType;

    fn fun_struct_complex_type(&self, complex_type: StructComplexType) -> StructComplexType;

    fn fun_tuple_complex_type(&self, complex_type: TupleComplexType) -> TupleComplexType;

    fn fun_tuple_type(&self, tuple_type: TupleType) -> TupleType;

    fn fun_list_complex_type(&self, list_complex_type: ListOfObjectType) -> ListOfObjectType;

    fn fun_list_string_type(&self, list_string_type: ListOfStringType) -> ListOfStringType;

    fn fun_enum_with_only_literals(
        &self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals;

    fn fun_simple_enum_type(&self, simple_enum_type: SimpleEnumType) -> SimpleEnumType;

    fn fun_complex_enum_type(&self, complex_enum_type: ComplexEnumType) -> ComplexEnumType;

    fn fun_simple_struct_type(&self, simple_struct_type: SimpleStructType) -> SimpleStructType;

    fn fun_result_type(&self, result_type: Result<String, String>) -> Result<String, String>;
    // TODO: Add multimodal, unstructured types test cases
}

struct FooAgentImpl {
    bar_agent: Box<dyn BarAgent>,
}

#[agent_implementation]
impl FooAgent for FooAgentImpl {
    async fn new(opt_string: Option<String>) -> Self {
        let bar_agent = BarAgent::get(opt_string.unwrap_or_else(|| "default_id".to_string())).await;

        FooAgentImpl {
            bar_agent: Box::new(bar_agent),
        }
    }

    fn fun_all(
        &self,
        struct_complex_type: StructComplexType,
        union_complex_type: EnumComplexType,
        f64_type: f64,
        f32_type: f32,
        u32_type: u32,
        i32_type: i32,
        ui6_type: u16,
        i16_type: i16,
        i8_type: i8,
        u8_type: u8,
        string_type: String,
        bool_type: bool,
        map_type: MapType,
        tuple_complex_type: TupleComplexType,
        tuple_type: TupleType,
        list_complex_type: ListOfObjectType,
        list_string_type: ListOfStringType,
        enum_with_only_literals: EnumWithOnlyLiterals,
        simple_enum_type: SimpleEnumType,
        complex_enum_type: ComplexEnumType,
        simple_struct_type: SimpleStructType,
        struct_with_single_field: StructWithSingleField,
        optional_string_type: OptionalStringType,
    ) -> String {
        self.bar_agent.fun_all(
            struct_complex_type,
            union_complex_type,
            f64_type,
            f32_type,
            u32_type,
            i32_type,
            ui6_type,
            i16_type,
            i8_type,
            u8_type,
            string_type,
            bool_type,
            map_type,
            tuple_complex_type,
            tuple_type,
            list_complex_type,
            list_string_type,
            enum_with_only_literals,
            simple_enum_type,
            complex_enum_type,
            simple_struct_type,
            struct_with_single_field,
            optional_string_type,
        )
    }

    fn fun_optional(
        &self,
        param1: Option<String>,
        param2: Option<StructComplexType>,
    ) -> Option<String> {
        self.bar_agent.fun_optional(param1, param2)
    }

    fn fun_no_return(&self, text: String) {
        self.bar_agent.fun_no_return(text);
    }

    fn fun_number(&self, number_type: f64) -> f64 {
        self.bar_agent.fun_number(number_type)
    }

    fn fun_string(&self, string_type: String) -> String {
        self.bar_agent.fun_string(string_type)
    }

    fn fun_boolean(&self, boolean_type: bool) -> bool {
        self.bar_agent.fun_boolean(boolean_type)
    }

    fn fun_map(&self, map_type: MapType) -> MapType {
        self.bar_agent.fun_map(map_type)
    }

    fn fun_struct_complex_type(&self, complex_type: StructComplexType) -> StructComplexType {
        self.bar_agent.fun_struct_complex_type(complex_type)
    }

    fn fun_tuple_complex_type(&self, complex_type: TupleComplexType) -> TupleComplexType {
        self.bar_agent.fun_tuple_complex_type(complex_type)
    }

    fn fun_tuple_type(&self, tuple_type: TupleType) -> TupleType {
        self.bar_agent.fun_tuple_type(tuple_type)
    }

    fn fun_list_complex_type(&self, list_complex_type: ListOfObjectType) -> ListOfObjectType {
        self.bar_agent.fun_list_complex_type(list_complex_type)
    }

    fn fun_list_string_type(&self, list_string_type: ListOfStringType) -> ListOfStringType {
        self.bar_agent.fun_list_string_type(list_string_type)
    }

    fn fun_enum_with_only_literals(
        &self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals {
        self.bar_agent.fun_enum_with_only_literals(enum_with_only_literals)
    }

    fn fun_simple_enum_type(&self, simple_enum_type: SimpleEnumType) -> SimpleEnumType {
        self.bar_agent.fun_simple_enum_type(simple_enum_type)
    }

    fn fun_complex_enum_type(&self, complex_enum_type: ComplexEnumType) -> ComplexEnumType {
        self.bar_agent.fun_complex_enum_type(complex_enum_type)
    }

    fn fun_simple_struct_type(&self, simple_struct_type: SimpleStructType) -> SimpleStructType {
        self.bar_agent.fun_simple_struct_type(simple_struct_type)
    }

    fn fun_result_type(&self, result_type: Result<String, String>) -> Result<String, String> {
        self.bar_agent.fun_result_type(result_type)
    }
}

#[agent_definition]
trait BarAgent {
    async fn new(opt_string: Option<String>) -> Self;

    fn fun_all(
        &self,
        struct_complex_type: StructComplexType,
        union_complex_type: EnumComplexType,
        f64_type: f64,
        f32_type: f32,
        u32_type: u32,
        i32_type: i32,
        ui6_type: u16,
        i16_type: i16,
        i8_type: i8,
        u8_type: u8,
        string_type: String,
        bool_type: bool,
        map_type: MapType,
        tuple_complex_type: TupleComplexType,
        tuple_type: TupleType,
        list_complex_type: ListOfObjectType,
        list_string_type: ListOfStringType,
        enum_with_only_literals: EnumWithOnlyLiterals,
        simple_enum_type: SimpleEnumType,
        complex_enum_type: ComplexEnumType,
        simple_struct_type: SimpleStructType,
        struct_with_single_field: StructWithSingleField,
        optional_string_type: OptionalStringType,
    ) -> String;

    fn fun_optional(
        &self,
        param1: Option<String>,
        param2: Option<StructComplexType>,
    ) -> Option<String>;

    fn fun_no_return(&self, text: String);

    fn fun_number(&self, number_type: f64) -> f64;

    fn fun_string(&self, string_type: String) -> String;

    fn fun_boolean(&self, boolean_type: bool) -> bool;

    fn fun_map(&self, map_type: MapType) -> MapType;

    fn fun_struct_complex_type(&self, complex_type: StructComplexType) -> StructComplexType;

    fn fun_tuple_complex_type(&self, complex_type: TupleComplexType) -> TupleComplexType;

    fn fun_tuple_type(&self, tuple_type: TupleType) -> TupleType;

    fn fun_list_complex_type(&self, list_complex_type: ListOfObjectType) -> ListOfObjectType;

    fn fun_list_string_type(&self, list_string_type: ListOfStringType) -> ListOfStringType;

    fn fun_enum_with_only_literals(
        &self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals;

    fn fun_simple_enum_type(&self, simple_enum_type: SimpleEnumType) -> SimpleEnumType;

    fn fun_complex_enum_type(&self, complex_enum_type: ComplexEnumType) -> ComplexEnumType;

    fn fun_simple_struct_type(&self, simple_struct_type: SimpleStructType) -> SimpleStructType;

    fn fun_result_type(&self, result_type: Result<String, String>) -> Result<String, String>;

    // TODO: Add multimodal, unstructured types test cases
}

struct BarAgentImpl {
    id: String,
}

#[agent_implementation]
impl BarAgent for BarAgentImpl {
    async fn new(opt_string: Option<String>) -> Self {
        BarAgentImpl {
            id: opt_string.unwrap_or_else(|| "default_id".to_string()),
        }
    }

    fn fun_all(
        &self,
        _struct_complex_type: StructComplexType,
        _union_complex_type: EnumComplexType,
        _f64_type: f64,
        _f32_type: f32,
        _u32_type: u32,
        _i32_type: i32,
        _ui6_type: u16,
        _i16_type: i16,
        _i8_type: i8,
        _u8_type: u8,
        _string_type: String,
        _bool_type: bool,
        _map_type: MapType,
        _tuple_complex_type: TupleComplexType,
        _tuple_type: TupleType,
        _list_complex_type: ListOfObjectType,
        _list_string_type: ListOfStringType,
        _enum_with_only_literals: EnumWithOnlyLiterals,
        _simple_enum_type: SimpleEnumType,
        _complex_enum_type: ComplexEnumType,
        _simple_struct_type: SimpleStructType,
        _struct_with_single_field: StructWithSingleField,
        _optional_string_type: OptionalStringType,
    ) -> String {
        "success".to_string()
    }

    fn fun_optional(
        &self,
        param1: Option<String>,
        _param2: Option<StructComplexType>,
    ) -> Option<String> {
        param1
    }

    fn fun_no_return(&self, text: String) {
        println!("Hello, {}", text);
    }

    fn fun_number(&self, number_type: f64) -> f64 {
        number_type
    }

    fn fun_string(&self, string_type: String) -> String {
        string_type
    }

    fn fun_boolean(&self, boolean_type: bool) -> bool {
        boolean_type
    }

    fn fun_map(&self, map_type: MapType) -> MapType {
        map_type
    }

    fn fun_struct_complex_type(&self, complex_type: StructComplexType) -> StructComplexType {
        complex_type
    }

    fn fun_tuple_complex_type(&self, complex_type: TupleComplexType) -> TupleComplexType {
        complex_type
    }

    fn fun_tuple_type(&self, tuple_type: TupleType) -> TupleType {
        tuple_type
    }

    fn fun_list_complex_type(&self, list_complex_type: ListOfObjectType) -> ListOfObjectType {
        list_complex_type
    }

    fn fun_list_string_type(&self, list_string_type: ListOfStringType) -> ListOfStringType {
        list_string_type
    }

    fn fun_enum_with_only_literals(
        &self,
        enum_with_only_literals: EnumWithOnlyLiterals,
    ) -> EnumWithOnlyLiterals {
        enum_with_only_literals
    }

    fn fun_simple_enum_type(&self, simple_enum_type: SimpleEnumType) -> SimpleEnumType {
        simple_enum_type
    }

    fn fun_complex_enum_type(&self, complex_enum_type: ComplexEnumType) -> ComplexEnumType {
        complex_enum_type
    }

    fn fun_simple_struct_type(&self, simple_struct_type: SimpleStructType) -> SimpleStructType {
        simple_struct_type
    }

    fn fun_result_type(&self, result_type: Result<String, String>) -> Result<String, String> {
        result_type
    }
}
