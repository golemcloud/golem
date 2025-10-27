use golem_wasm::analysis::analysed_type::{str, u32, u64};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::WitValue;
use golem_wasm::{Value, WitType};

pub trait AgentArg: ToValue + FromWitValue + ToWitType {
    fn to_value(&self) -> golem_wasm::Value {
        ToValue::to_value(self)
    }

    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        FromWitValue::from_wit_value(value)
    }

    fn to_wit_value(&self) -> WitValue {
        let value = ToValue::to_value(self);
        WitValue::from(value)
    }

    fn get_wit_type() -> WitType
    where
        Self: Sized,
    {
        <Self as ToWitType>::get_wit_type()
    }
}

impl<T: ToValue + FromWitValue + ToWitType> AgentArg for T {}

pub trait ToValue {
    fn to_value(&self) -> golem_wasm::Value;
}

pub trait FromValue {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String>
    where
        Self: Sized;
}

impl ToValue for String {
    fn to_value(&self) -> golem_wasm::Value {
        golem_wasm::Value::String(self.clone())
    }
}

impl ToValue for u32 {
    fn to_value(&self) -> Value {
        golem_wasm::Value::U32(*self)
    }
}

impl ToValue for bool {
    fn to_value(&self) -> Value {
        golem_wasm::Value::Bool(*self)
    }
}

impl ToValue for u64 {
    fn to_value(&self) -> Value {
        golem_wasm::Value::U64(*self)
    }
}

impl FromValue for String {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::String(s) => Ok(s),
            _ => Err("Expected a String value".to_string()),
        }
    }
}

impl FromValue for u32 {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::U32(n) => Ok(n),
            _ => Err("Expected a u32 value".to_string()),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Bool(b) => Ok(b),
            _ => Err("Expected a bool value".to_string()),
        }
    }
}

impl FromValue for u64 {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::U64(n) => Ok(n),
            _ => Err("Expected a u64 value".to_string()),
        }
    }
}

pub trait ToWitValue {
    fn to_wit_value(&self) -> golem_wasm::WitValue;
}

impl<T: ToValue> ToWitValue for T {
    fn to_wit_value(&self) -> WitValue {
        let value = self.to_value();
        WitValue::from(value)
    }
}

pub trait ToWitType {
    fn get_wit_type() -> WitType;
}

impl ToWitType for String {
    fn get_wit_type() -> WitType {
        let analysed_type = str();
        WitType::from(analysed_type)
    }
}

impl ToWitType for u32 {
    fn get_wit_type() -> WitType {
        let analysed_type: AnalysedType = u32();
        WitType::from(analysed_type)
    }
}

impl ToWitType for u64 {
    fn get_wit_type() -> WitType {
        let analysed_type = u64();
        WitType::from(analysed_type)
    }
}

pub trait FromWitValue {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized;
}

impl FromWitValue for String {
    fn from_wit_value(value: WitValue) -> Result<Self, String> {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::String(s) => Ok(s),
            _ => Err("Expected a String WitValue".to_string()),
        }
    }
}

impl FromWitValue for u32 {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::U32(n) => Ok(n),
            _ => Err("Expected a u32 WitValue".to_string()),
        }
    }
}

impl FromWitValue for bool {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::Bool(b) => Ok(b),
            _ => Err("Expected a bool WitValue".to_string()),
        }
    }
}

impl FromWitValue for u64 {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::U64(n) => Ok(n),
            _ => Err("Expected a u32 WitValue".to_string()),
        }
    }
}

impl FromWitValue for Vec<WitValue> {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::List(list) => Ok(list.into_iter().map(WitValue::from).collect()),
            _ => Err("Expected a List WitValue".to_string()),
        }
    }
}

impl<T: FromWitValue> FromWitValue for Vec<T> {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::List(list) => list
                .into_iter()
                .map(|v| T::from_wit_value(WitValue::from(v)))
                .collect(),
            _ => Err("Expected a List WitValue".to_string()),
        }
    }
}

impl<T: FromWitValue> FromWitValue for Option<T> {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);

        match value {
            golem_wasm::Value::Option(Some(v)) => {
                T::from_wit_value(WitValue::from(v.as_ref().clone())).map(Some)
            }
            golem_wasm::Value::Option(None) => Ok(None),
            _ => Err("Expected an Option WitValue".to_string()),
        }
    }
}

impl FromWitValue for golem_wasm::Value {
    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(golem_wasm::Value::from(value))
    }
}
