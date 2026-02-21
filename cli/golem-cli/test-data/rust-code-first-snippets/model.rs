use std::collections::Bound;

use std::collections::HashMap;

use golem_rust::{AllowedLanguages, AllowedMimeTypes, MultimodalSchema, Schema};

#[derive(Schema)]
pub struct AllPrimitives {
    pub u8v: u8,
    pub u16v: u16,
    pub u32v: u32,
    pub u64v: u64,
    pub i8v: i8,
    pub i16v: i16,
    pub i32v: i32,
    pub i64v: i64,
    pub f32v: f32,
    pub f64v: f64,
    pub boolv: bool,
    pub charv: char,
    pub stringv: String,
}

#[derive(Schema)]
pub struct OptionResultBound {
    pub option_u8: Option<u8>,
    pub option_str: Option<String>,
    pub res_ok: Result<String, String>,
    pub res_num_err: Result<u32, String>,
    pub res_unit_ok: Result<String, String>,
    pub res_unit_err: Result<String, String>,
    pub bound_u8: Bound<u8>,
    pub bound_str: Bound<String>,
}

#[derive(Schema)]
pub struct Tuples {
    pub pair: (String, f64),
    pub triple: (String, f64, bool),
    pub mixed: (i8, u16, f32),
}

#[derive(Schema)]
pub struct Collections {
    pub list_u8: Vec<u8>,
    pub list_str: Vec<String>,
    pub map_num: HashMap<String, f64>,
    pub map_text: HashMap<i32, String>,
}

#[derive(Schema)]
pub struct SimpleStruct {
    pub name: String,
    pub value: f64,
    pub flag: bool,
    pub symbol: char,
}

#[derive(Schema)]
pub struct NestedStruct {
    pub id: String,
    pub simple: SimpleStruct,
    pub list: Vec<SimpleStruct>,
    pub map: HashMap<String, f64>,
    pub option: Option<String>,
    pub result: Result<String, String>,
}

#[derive(Schema)]
pub enum SimpleEnum {
    U8(u8),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Char(char),
    String(String),
    Struct(SimpleStruct),
    Unit,
}

#[derive(Schema)]
pub enum EnumWithOnlyLiterals {
    A,
    B,
    C,
}

#[derive(Schema)]
pub enum EnumWithCollections {
    Vec(Vec<u8>),
    Map(HashMap<String, f64>),
    Tuple((String, f64)),
    Bound(Bound<u8>),
}

#[derive(Schema)]
pub enum ComplexEnum {
    Primitive(SimpleEnum),
    Struct(NestedStruct),
    ListOfStructs(Vec<SimpleStruct>),
    Option(Option<String>),
    Result(Result<String, String>),
    Map(HashMap<String, f64>),
    Bound(Bound<String>),
    Tuple((String, f64, bool)),
    UnitA,
    UnitB,
}

#[derive(Schema)]
pub struct ComplexStruct {
    pub primitives: AllPrimitives,
    pub options_results_bounds: OptionResultBound,
    pub tuples: Tuples,
    pub collections: Collections,
    pub simple_struct: SimpleStruct,
    pub nested_struct: NestedStruct,
    pub enum_simple: SimpleEnum,
    pub enum_collections: EnumWithCollections,
    pub enum_complex: ComplexEnum,
}

#[derive(MultimodalSchema)]
pub enum TextImageData {
    Text(String),
    Image(Vec<u8>),
    Data(Data),
}

#[derive(Schema)]
pub struct Data {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, AllowedLanguages)]
pub enum MyLang {
    #[code("en")]
    En,
    De,
}

#[derive(Debug, Clone, AllowedMimeTypes)]
pub enum MyMimeType {
    #[mime_type("text/plain")]
    PlainText,
    #[mime_type("image/png")]
    PngImage,
}
