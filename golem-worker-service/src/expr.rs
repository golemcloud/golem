use std::collections::HashSet;
use std::str::FromStr;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize, Serializer};

use crate::parser::expr_parser::ExprParser;
use crate::parser::{GolemParser, ParseError};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum Expr {
    Request(),
    WorkerResponse(),
    SelectField(Box<Expr>, String),
    SelectIndex(Box<Expr>, usize),
    Sequence(Vec<Expr>),
    Record(Vec<(String, Box<Expr>)>),
    Literal(String),
    PathVar(String),
    Concat(Vec<Expr>),
    Not(Box<Expr>),
    GreaterThan(Box<Expr>, Box<Expr>),
    GreaterThanOrEqualTo(Box<Expr>, Box<Expr>),
    LessThanOrEqualTo(Box<Expr>, Box<Expr>),
    EqualTo(Box<Expr>, Box<Expr>),
    LessThan(Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Option0(Option<Box<Expr>>),
    Result0(Result<Box<Expr>, Box<Expr>>),
    PatternMatch(Box<Expr>, Box<Expr>),
    ConstructorPattern
}


// A constructor pattern by itself is an expr,
// as it may not be always associated with match expr
// or a simple if statement. if ok(res) == foo then res else bar
enum ConstructorPattern {
    WildCard,
    As(String, Box<ConstructorPattern>),
    Constructor(String, Vec<ConstructorPattern>),
    Literal(Box<Expr>)
}

impl Expr {
    pub fn is_literal(&self) -> bool {
        match self {
            Expr::Literal(_) => true,
            Expr::Concat(vec) => vec.iter().all(|x| x.is_literal()),
            _ => false,
        }
    }

    // A primitive string can be converted to Expr. Not that if the string is already as complex as yaml or json, use functions such as Expr::from_yaml_value, Expr::from_json_value
    pub fn from_primitive_string(input: &str) -> Result<Expr, ParseError> {
        let expr_parser = ExprParser {};
        expr_parser.parse(input)
    }

    // A json can be mapped to Expr. Example: Json::Record can be mmpped to Expr::Record
    pub fn from_json_value(input: &serde_json::Value) -> Result<Expr, ParseError> {
        match input {
            serde_json::Value::Bool(bool) => Ok(Expr::Literal(bool.to_string())),
            serde_json::Value::Number(number) => Ok(Expr::Literal(number.to_string())),
            serde_json::Value::String(string) => Expr::from_primitive_string(string),
            serde_json::Value::Array(sequence) => {
                let mut exprs: Vec<Expr> = vec![];

                for s in sequence {
                    let expr: Expr = Expr::from_json_value(s)?;
                    exprs.push(expr);
                }

                Ok(Expr::Sequence(exprs))
            }
            serde_json::Value::Object(mapping) => {
                let mut tuple_vec: Vec<(String, Box<Expr>)> = vec![];

                for (k, v) in mapping {
                    let value_expr: Expr = Expr::from_json_value(v)?;

                    tuple_vec.push((k.clone(), Box::new(value_expr)))
                }
                Ok(Expr::Record(tuple_vec))
            }

            serde_json::Value::Null => {
                Err(ParseError::Message("Json Null not implemented".to_string()))
            }
        }
    }

    pub fn to_json_value(&self) -> Result<serde_json::Value, String> {
        fn go(expr: &Expr) -> Result<InternalValue<serde_json::Value>, String> {
            match expr {
                Expr::SelectField(expr0, field_name) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_json::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_json::Value::String(format!(
                                "{}.{}",
                                expr_string,
                                field_name
                            )))),
                        _ => Err("Invalid selection of field. {}. Example of a valid selection: request.body.users".into())
                    }
                }

                Expr::SelectIndex(expr0, index) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_json::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_json::Value::String(format!(
                                "{}[{}]",
                                expr_string,
                                index
                            )))),
                        _ => Err("Invalid selection of index. Example of a valid selection: request.body.users[1]".into())
                    }
                }

                Expr::Request() => Ok(InternalValue::Interpolated(serde_json::Value::String(
                    "request".to_string(),
                ))),

                // TODO; only worker as response
                Expr::WorkerResponse() => Ok(InternalValue::Interpolated(
                    serde_json::Value::String("worker.response".to_string()),
                )),

                Expr::Record(values) => {
                    let mut mapping = serde_json::Map::new();

                    for (key, value) in values {
                        let key_yaml = key.clone();
                        let value_json = Expr::to_json_value(value)?;
                        mapping.insert(key_yaml, value_json);
                    }

                    Ok(InternalValue::NonInterpolated(serde_json::Value::Object(
                        mapping,
                    )))
                }
                Expr::Sequence(values) => {
                    let mut vs: Vec<serde_json::Value> = vec![];
                    for value in values {
                        let v = value.to_json_value()?;
                        vs.push(v);
                    }
                    Ok(InternalValue::NonInterpolated(serde_json::Value::Array(vs)))
                }
                Expr::Literal(value) => Ok(InternalValue::NonInterpolated(
                    serde_json::Value::String(value.clone()),
                )),
                Expr::PathVar(value) => Ok(InternalValue::Interpolated(serde_json::Value::String(
                    value.clone(),
                ))),
                Expr::Concat(values) => {
                    let mut vs: Vec<String> = vec![];
                    for value in values {
                        let v = go(value)?;
                        match v {
                            InternalValue::Interpolated(serde_json::Value::String(v)) => {
                                vs.push(format!("${{{}}}", v));
                            }
                            InternalValue::NonInterpolated(serde_json::Value::String(v)) => {
                                vs.push(v);
                            }
                            _ => return Err("Not supported type".into()),
                        }
                    }
                    Ok(InternalValue::NonInterpolated(serde_json::Value::String(
                        vs.join(""),
                    )))
                }
                Expr::Not(value) => {
                    let v = value.to_json_value()?;
                    match v {
                        serde_json::Value::String(v) => Ok(InternalValue::Interpolated(
                            serde_json::Value::String(format!("!{}", v)),
                        )),
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}>{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}>={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::EqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}=={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}<{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_json::Value::String(v1), serde_json::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_json::Value::String(
                                format!("{}<={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::Cond(pred, value1, value2) => {
                    let p = go(pred)?;
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    // FIXME: How did we handle encoding of elseif ?
                    // It's good to remove this encoding as it doesn't give much value add.
                    // once parsed, we can pipe the input directly to the datastore
                    match (p.unwrap(), v1.unwrap(), v2.unwrap()) {
                        (
                            serde_json::Value::String(p),
                            serde_json::Value::String(v1),
                            serde_json::Value::String(v2),
                        ) => Ok(InternalValue::Interpolated(serde_json::Value::String(
                            format!("if ({}) then {} else {}", p, v1, v2),
                        ))),
                        _ => Err("Not supported type".into()),
                    }
                }
            }
        }

        let internal_result = go(self)?;

        match internal_result {
            InternalValue::Interpolated(serde_json::Value::String(string)) => {
                Ok(serde_json::Value::String(format!("${{{}}}", string)))
            }
            InternalValue::NonInterpolated(value) => Ok(value),
            _ => Err("Cannot write back the expr".into()),
        }
    }

    // A  yaml can be mapped to Expr. Example: Yaml::Mapping can be mmpped to Expr::Record
    pub fn from_yaml_value(input: &serde_yaml::Value) -> Result<Expr, ParseError> {
        match input {
            serde_yaml::Value::Bool(bool) => Ok(Expr::Literal(bool.to_string())),
            serde_yaml::Value::Number(number) => Ok(Expr::Literal(number.to_string())),
            serde_yaml::Value::String(string) => Expr::from_primitive_string(string),
            serde_yaml::Value::Sequence(sequence) => {
                let mut exprs: Vec<Expr> = vec![];

                for s in sequence {
                    let expr: Expr = Expr::from_yaml_value(s)?;
                    exprs.push(expr);
                }

                Ok(Expr::Sequence(exprs))
            }
            serde_yaml::Value::Mapping(mapping) => {
                let mut tuple_vec: Vec<(String, Box<Expr>)> = vec![];

                for (k, v) in mapping {
                    let key_expr = Expr::from_yaml_value(k)?;

                    let key = match key_expr {
                        Expr::Literal(value) => value,
                        _ => {
                            return Err(ParseError::Message(
                                format!(
                                    "The key {:?} cannot be a complex structure but a primitive value",
                                    k
                                )
                                    .to_string(),
                            ));
                        }
                    };
                    let value_expr: Expr = Expr::from_yaml_value(v)?;

                    tuple_vec.push((key, Box::new(value_expr)))
                }
                Ok(Expr::Record(tuple_vec))
            }
            serde_yaml::Value::Tagged(_) => {
                Err(ParseError::Message("Tagged not implemented".to_string()))
            }
            serde_yaml::Value::Null => Err(ParseError::Message("Null not implemented".to_string())),
        }
    }

    pub fn to_yaml_value(&self) -> Result<serde_yaml::Value, String> {
        fn go(expr: &Expr) -> Result<InternalValue<serde_yaml::Value>, String> {
            match expr {
                Expr::SelectField(expr0, field_name) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_yaml::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(format!(
                                "{}.{}",
                                expr_string,
                                field_name
                            )))),
                        _ => Err("Invalid selection of field. {}. Example of a valid selection: request.body.users".into())
                    }
                }

                Expr::SelectIndex(expr0, index) => {
                    let expr0: &Expr = expr0;
                    let expr = go(expr0)?;

                    match expr.unwrap() {
                        serde_yaml::Value::String(expr_string) =>
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(format!(
                                "{}[{}]",
                                expr_string,
                                index
                            )))),
                        _ => Err("Invalid selection of index. Example of a valid selection: request.body.users[1]".into())
                    }
                }

                Expr::Request() => Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                    "request".to_string(),
                ))),

                // TODO; only worker as response
                Expr::WorkerResponse() => Ok(InternalValue::Interpolated(
                    serde_yaml::Value::String("worker.response".to_string()),
                )),

                Expr::Record(values) => {
                    let mut mapping = serde_yaml::mapping::Mapping::new();

                    for (key, value) in values {
                        let key_yaml = serde_yaml::Value::String(key.clone());
                        let value_yaml = Expr::to_yaml_value(value)?;
                        mapping.insert(key_yaml, value_yaml);
                    }

                    Ok(InternalValue::NonInterpolated(serde_yaml::Value::Mapping(
                        mapping,
                    )))
                }
                Expr::Sequence(values) => {
                    let mut vs: Vec<serde_yaml::Value> = vec![];
                    for value in values {
                        let v = value.to_yaml_value()?;
                        vs.push(v);
                    }
                    Ok(InternalValue::NonInterpolated(serde_yaml::Value::Sequence(
                        vs,
                    )))
                }
                Expr::Literal(value) => Ok(InternalValue::NonInterpolated(
                    serde_yaml::Value::String(value.clone()),
                )),
                Expr::PathVar(value) => Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                    value.clone(),
                ))),
                Expr::Concat(values) => {
                    let mut vs: Vec<String> = vec![];
                    for value in values {
                        let v = go(value)?;
                        match v {
                            InternalValue::Interpolated(serde_yaml::Value::String(v)) => {
                                vs.push(format!("${{{}}}", v));
                            }
                            InternalValue::NonInterpolated(serde_yaml::Value::String(v)) => {
                                vs.push(v);
                            }
                            _ => return Err("Not supported type".into()),
                        }
                    }
                    Ok(InternalValue::NonInterpolated(serde_yaml::Value::String(
                        vs.join(""),
                    )))
                }
                Expr::Not(value) => {
                    let v = value.to_yaml_value()?;
                    match v {
                        serde_yaml::Value::String(v) => Ok(InternalValue::Interpolated(
                            serde_yaml::Value::String(format!("!{}", v)),
                        )),
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_yaml::Value::String(v1), serde_yaml::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                                format!("{}>{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::GreaterThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_yaml::Value::String(v1), serde_yaml::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                                format!("{}>={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::EqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_yaml::Value::String(v1), serde_yaml::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                                format!("{}=={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThan(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_yaml::Value::String(v1), serde_yaml::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                                format!("{}<{}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::LessThanOrEqualTo(value1, value2) => {
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    match (v1.unwrap(), v2.unwrap()) {
                        (serde_yaml::Value::String(v1), serde_yaml::Value::String(v2)) => {
                            Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                                format!("{}<={}", v1, v2),
                            )))
                        }
                        _ => Err("Not supported type".into()),
                    }
                }
                Expr::Cond(pred, value1, value2) => {
                    let p = go(pred)?;
                    let v1 = go(value1)?;
                    let v2 = go(value2)?;
                    // FIXME: How did we handle encoding of elseif ?
                    // It's good to remove this encoding as it doesn't give much value add.
                    // once parsed, we can pipe the input directly to the datastore
                    match (p.unwrap(), v1.unwrap(), v2.unwrap()) {
                        (
                            serde_yaml::Value::String(p),
                            serde_yaml::Value::String(v1),
                            serde_yaml::Value::String(v2),
                        ) => Ok(InternalValue::Interpolated(serde_yaml::Value::String(
                            format!("if ({}) then {} else {}", p, v1, v2),
                        ))),
                        _ => Err("Not supported type".into()),
                    }
                }
            }
        }

        let internal_result = go(self)?;

        match internal_result {
            InternalValue::Interpolated(serde_yaml::Value::String(string)) => {
                Ok(serde_yaml::Value::String(format!("${{{}}}", string)))
            }
            InternalValue::NonInterpolated(value) => Ok(value),
            _ => Err("Cannot write back the expr".into()),
        }
    }

    pub fn to_string(&self) -> Result<String, String> {
        let v = self.to_json_value();
        match v {
            Ok(serde_json::Value::String(v)) => Ok(v),
            _ => Err("Not supported type".to_string()),
        }
    }

    pub fn get_vars(&self) -> HashSet<String> {
        let mut vars: HashSet<String> = HashSet::new();
        match self {
            Expr::Request() => {}
            Expr::WorkerResponse() => {}
            sf @ Expr::SelectField(_, _) => {
                fn go(expr: &Expr) -> Option<String> {
                    match expr {
                        Expr::Request() => Some("request".to_string()),
                        Expr::WorkerResponse() => Some("worker.response".to_string()),
                        Expr::SelectField(expr, field) => {
                            go(expr).map(|v| format!("{}.{}", v, field))
                        }
                        Expr::SelectIndex(expr, index) => {
                            go(expr).map(|v: String| format!("{}[{}]", v, index))
                        }
                        _ => None,
                    }
                }
                let sfv = go(sf);
                if let Some(v) = sfv {
                    vars.insert(v);
                }
            }

            sf @ Expr::SelectIndex(_, _) => {
                fn go(expr: &Expr) -> Option<String> {
                    match expr {
                        Expr::Request() => Some("request".to_string()),
                        Expr::WorkerResponse() => Some("worker.response".to_string()),
                        Expr::SelectField(expr, field) => {
                            go(expr).map(|v| format!("{}.{}", v, field))
                        }
                        Expr::SelectIndex(expr, index) => {
                            go(expr).map(|v: String| format!("{}[{}]", v, index))
                        }
                        _ => None,
                    }
                }
                let sfv = go(sf);
                if let Some(v) = sfv {
                    vars.insert(v);
                }
            }

            Expr::Record(values) => {
                let vs = values
                    .iter()
                    .flat_map(|(_, v)| v.get_vars())
                    .collect::<HashSet<String>>();

                vars.extend(vs);
            }
            Expr::Sequence(values) => {
                let vs = values
                    .iter()
                    .flat_map(|v| v.get_vars())
                    .collect::<HashSet<String>>();

                vars.extend(vs);
            }
            Expr::PathVar(v) => {
                vars.insert(v.clone());
            }
            Expr::Literal(_) => {}
            Expr::Concat(values) => {
                let vs = values
                    .iter()
                    .flat_map(|v| v.get_vars())
                    .collect::<HashSet<String>>();

                vars.extend(vs);
            }
            Expr::Not(value) => {
                vars.extend(value.get_vars());
            }
            Expr::GreaterThan(value1, value2) => {
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
            Expr::GreaterThanOrEqualTo(value1, value2) => {
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
            Expr::EqualTo(value1, value2) => {
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
            Expr::LessThan(value1, value2) => {
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
            Expr::LessThanOrEqualTo(value1, value2) => {
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
            Expr::Cond(pred, value1, value2) => {
                vars.extend(pred.get_vars());
                vars.extend(value1.get_vars());
                vars.extend(value2.get_vars());
            }
        }

        vars
    }
}

impl FromStr for Expr {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let expr_parser = ExprParser {};
        expr_parser.parse(s)
    }
}

impl<'de> Deserialize<'de> for Expr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match Expr::from_json_value(&value) {
            Ok(expr) => Ok(expr),
            Err(message) => Err(serde::de::Error::custom(message.to_string())),
        }
    }
}

impl Serialize for Expr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.to_json_value() {
            Ok(value) => serde_json::Value::serialize(value, serializer),
            Err(error) => Err(serde::ser::Error::custom(error.to_string())),
        }
    }
}

enum InternalValue<T> {
    Interpolated(T),
    NonInterpolated(T),
}

impl<T> InternalValue<T> {
    fn unwrap(&self) -> &T {
        match self {
            InternalValue::Interpolated(value) => value,
            InternalValue::NonInterpolated(value) => value,
        }
    }
}
