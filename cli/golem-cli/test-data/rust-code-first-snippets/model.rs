use std::collections::HashMap;

type BooleanType = bool;
type StringType = String;
type NumberType = f64;

pub struct StructWithSingleField {
    pub n: f64,
}

pub struct SimpleStructType {
    pub a: String,
    pub b: f64,
    pub c: bool,
}

pub enum ComplexEnumType {
    A(String),
    B(f64),
    C(bool),
    D(SimpleEnumType),
    E(SimpleStructType),
    F(ListOfStringType),
    G(TupleType),
    H(SimpleStructType),
    I,
    J,
}

pub enum SimpleEnumType {
    Number(f64),
    String(String),
    Boolean(bool),
    SimpleStruct(SimpleStructType),
}

pub enum EnumWithOnlyLiterals {
    Foo,
    Bar,
    Baz,
}

pub type ListOfStringType = Vec<String>;
pub type ListOfSructType = Vec<SimpleStructType>;
pub type TupleType = (String, f64, bool);
pub type TupleComplexType = (String, f64, SimpleStructType);
pub type MapType = HashMap<String, f64>;

pub struct StructComplexType {
    pub a: String,
    pub b: f64,
    pub c: bool,
    pub d: SimpleStructType,
    pub e: SimpleEnumType,
    pub f: ListOfStringType,
    pub g: ListOfSructType,
    pub h: TupleType,
    pub i: TupleComplexType,
    pub j: MapType,
    pub k: SimpleStructType,
    pub l: OptionalStringType,
}

pub enum EnumComplexType {
    Number(f64),
    String(String),
    Boolean(bool),
    ObjectComplex(StructComplexType),
    Union(SimpleEnumType),
    Tuple(TupleType),
    TupleComplex(TupleComplexType),
    Simple(SimpleStructType),
    Map(MapType),
    List(ListOfStringType),
    ListComplex(ListOfSructType),
    Optional(OptionalStringType),
}

pub type OptionalStringType = Option<String>;
pub type ResultType = Result<String, String>;
