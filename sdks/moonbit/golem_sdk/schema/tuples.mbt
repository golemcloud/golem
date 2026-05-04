///|
pub impl[T0 : HasElementSchema, T1 : HasElementSchema] HasElementSchema for (
  T0,
  T1,
) with element_schema() {
  make_tuple_schema([T0::element_schema(), T1::element_schema()])
}

///|
pub impl[T0 : FromExtractor, T1 : FromExtractor] FromExtractor for (T0, T1) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  (v0, v1)
}

///|
pub impl[T0 : FromExtractor, T1 : FromExtractor] FromElementValue for (T0, T1) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[T0 : ToElementValue, T1 : ToElementValue] ToElementValue for (T0, T1) with to_element_value(
  self,
) {
  let (v0, v1) = self
  make_tuple_value([T0::to_element_value(v0), T1::to_element_value(v1)])
}

///|
pub impl[T0 : HasElementSchema, T1 : HasElementSchema, T2 : HasElementSchema] HasElementSchema for (
  T0,
  T1,
  T2,
) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
  ])
}

///|
pub impl[T0 : FromExtractor, T1 : FromExtractor, T2 : FromExtractor] FromExtractor for (
  T0,
  T1,
  T2,
) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  (v0, v1, v2)
}

///|
pub impl[T0 : FromExtractor, T1 : FromExtractor, T2 : FromExtractor] FromElementValue for (
  T0,
  T1,
  T2,
) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[T0 : ToElementValue, T1 : ToElementValue, T2 : ToElementValue] ToElementValue for (
  T0,
  T1,
  T2,
) with to_element_value(self) {
  let (v0, v1, v2) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  (v0, v1, v2, v3)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3) with to_element_value(self) {
  let (v0, v1, v2, v3) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  (v0, v1, v2, v3, v4)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4) with to_element_value(self) {
  let (v0, v1, v2, v3, v4) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  (v0, v1, v2, v3, v4, v5)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5) with to_element_value(self) {
  let (v0, v1, v2, v3, v4, v5) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  (v0, v1, v2, v3, v4, v5, v6)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6) with to_element_value(self) {
  let (v0, v1, v2, v3, v4, v5, v6) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  (v0, v1, v2, v3, v4, v5, v6, v7)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7, T8) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
  T11 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
    T11::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  let v11 : T11 = extract_tuple_element(e, 11)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
  T11 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
    T11::to_element_value(v11),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
  T11 : HasElementSchema,
  T12 : HasElementSchema,
] HasElementSchema for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
    T11::element_schema(),
    T12::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  let v11 : T11 = extract_tuple_element(e, 11)
  let v12 : T12 = extract_tuple_element(e, 12)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
] FromElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12) with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
  T11 : ToElementValue,
  T12 : ToElementValue,
] ToElementValue for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12) with to_element_value(
  self,
) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
    T11::to_element_value(v11),
    T12::to_element_value(v12),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
  T11 : HasElementSchema,
  T12 : HasElementSchema,
  T13 : HasElementSchema,
] HasElementSchema for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
    T11::element_schema(),
    T12::element_schema(),
    T13::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
] FromExtractor for (T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13) with from_extractor(
  e,
) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  let v11 : T11 = extract_tuple_element(e, 11)
  let v12 : T12 = extract_tuple_element(e, 12)
  let v13 : T13 = extract_tuple_element(e, 13)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
] FromElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
  T11 : ToElementValue,
  T12 : ToElementValue,
  T13 : ToElementValue,
] ToElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
) with to_element_value(self) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
    T11::to_element_value(v11),
    T12::to_element_value(v12),
    T13::to_element_value(v13),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
  T11 : HasElementSchema,
  T12 : HasElementSchema,
  T13 : HasElementSchema,
  T14 : HasElementSchema,
] HasElementSchema for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
    T11::element_schema(),
    T12::element_schema(),
    T13::element_schema(),
    T14::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
  T14 : FromExtractor,
] FromExtractor for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  let v11 : T11 = extract_tuple_element(e, 11)
  let v12 : T12 = extract_tuple_element(e, 12)
  let v13 : T13 = extract_tuple_element(e, 13)
  let v14 : T14 = extract_tuple_element(e, 14)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
  T14 : FromExtractor,
] FromElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
  T11 : ToElementValue,
  T12 : ToElementValue,
  T13 : ToElementValue,
  T14 : ToElementValue,
] ToElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
) with to_element_value(self) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
    T11::to_element_value(v11),
    T12::to_element_value(v12),
    T13::to_element_value(v13),
    T14::to_element_value(v14),
  ])
}

///|
pub impl[
  T0 : HasElementSchema,
  T1 : HasElementSchema,
  T2 : HasElementSchema,
  T3 : HasElementSchema,
  T4 : HasElementSchema,
  T5 : HasElementSchema,
  T6 : HasElementSchema,
  T7 : HasElementSchema,
  T8 : HasElementSchema,
  T9 : HasElementSchema,
  T10 : HasElementSchema,
  T11 : HasElementSchema,
  T12 : HasElementSchema,
  T13 : HasElementSchema,
  T14 : HasElementSchema,
  T15 : HasElementSchema,
] HasElementSchema for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
  T15,
) with element_schema() {
  make_tuple_schema([
    T0::element_schema(),
    T1::element_schema(),
    T2::element_schema(),
    T3::element_schema(),
    T4::element_schema(),
    T5::element_schema(),
    T6::element_schema(),
    T7::element_schema(),
    T8::element_schema(),
    T9::element_schema(),
    T10::element_schema(),
    T11::element_schema(),
    T12::element_schema(),
    T13::element_schema(),
    T14::element_schema(),
    T15::element_schema(),
  ])
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
  T14 : FromExtractor,
  T15 : FromExtractor,
] FromExtractor for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
  T15,
) with from_extractor(e) {
  let v0 : T0 = extract_tuple_element(e, 0)
  let v1 : T1 = extract_tuple_element(e, 1)
  let v2 : T2 = extract_tuple_element(e, 2)
  let v3 : T3 = extract_tuple_element(e, 3)
  let v4 : T4 = extract_tuple_element(e, 4)
  let v5 : T5 = extract_tuple_element(e, 5)
  let v6 : T6 = extract_tuple_element(e, 6)
  let v7 : T7 = extract_tuple_element(e, 7)
  let v8 : T8 = extract_tuple_element(e, 8)
  let v9 : T9 = extract_tuple_element(e, 9)
  let v10 : T10 = extract_tuple_element(e, 10)
  let v11 : T11 = extract_tuple_element(e, 11)
  let v12 : T12 = extract_tuple_element(e, 12)
  let v13 : T13 = extract_tuple_element(e, 13)
  let v14 : T14 = extract_tuple_element(e, 14)
  let v15 : T15 = extract_tuple_element(e, 15)
  (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15)
}

///|
pub impl[
  T0 : FromExtractor,
  T1 : FromExtractor,
  T2 : FromExtractor,
  T3 : FromExtractor,
  T4 : FromExtractor,
  T5 : FromExtractor,
  T6 : FromExtractor,
  T7 : FromExtractor,
  T8 : FromExtractor,
  T9 : FromExtractor,
  T10 : FromExtractor,
  T11 : FromExtractor,
  T12 : FromExtractor,
  T13 : FromExtractor,
  T14 : FromExtractor,
  T15 : FromExtractor,
] FromElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
  T15,
) with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[
  T0 : ToElementValue,
  T1 : ToElementValue,
  T2 : ToElementValue,
  T3 : ToElementValue,
  T4 : ToElementValue,
  T5 : ToElementValue,
  T6 : ToElementValue,
  T7 : ToElementValue,
  T8 : ToElementValue,
  T9 : ToElementValue,
  T10 : ToElementValue,
  T11 : ToElementValue,
  T12 : ToElementValue,
  T13 : ToElementValue,
  T14 : ToElementValue,
  T15 : ToElementValue,
] ToElementValue for (
  T0,
  T1,
  T2,
  T3,
  T4,
  T5,
  T6,
  T7,
  T8,
  T9,
  T10,
  T11,
  T12,
  T13,
  T14,
  T15,
) with to_element_value(self) {
  let (v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15) = self
  make_tuple_value([
    T0::to_element_value(v0),
    T1::to_element_value(v1),
    T2::to_element_value(v2),
    T3::to_element_value(v3),
    T4::to_element_value(v4),
    T5::to_element_value(v5),
    T6::to_element_value(v6),
    T7::to_element_value(v7),
    T8::to_element_value(v8),
    T9::to_element_value(v9),
    T10::to_element_value(v10),
    T11::to_element_value(v11),
    T12::to_element_value(v12),
    T13::to_element_value(v13),
    T14::to_element_value(v14),
    T15::to_element_value(v15),
  ])
}
