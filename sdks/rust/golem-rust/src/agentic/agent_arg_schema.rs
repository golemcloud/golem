use crate::wasm_rpc::IntoValue;
use crate::wasm_rpc::WitType;
use crate::wasm_rpc::WitValue;

pub trait Schema: IntoValue + FromValue {
    fn to_value(self) -> golem_wasm::Value
    where
        Self: Sized,
    {
        IntoValue::into_value(self)
    }

    fn from_value(value: golem_wasm::Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        FromValue::from_value(value)
    }

    fn from_wit_value(value: WitValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        let value = golem_wasm::Value::from(value);
        FromValue::from_value(value)
    }

    fn to_wit_value(self) -> WitValue
    where
        Self: Sized,
    {
        let value = IntoValue::into_value(self);
        WitValue::from(value)
    }

    fn get_wit_type() -> WitType
    where
        Self: Sized,
    {
        let analysed_type = <Self as IntoValue>::get_type();
        WitType::from(analysed_type)
    }
}

impl<T: IntoValue + FromValue> Schema for T {}

pub trait FromValue {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String>
    where
        Self: Sized;
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

impl FromValue for i32 {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::S32(n) => Ok(n),
            _ => Err("Expected a i32 WitValue".to_string()),
        }
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::List(list) => list.into_iter().map(|v| T::from_value(v)).collect(),
            _ => Err("Expected a List WitValue".to_string()),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Option(Some(v)) => T::from_value(v.as_ref().clone()).map(Some),
            golem_wasm::Value::Option(None) => Ok(None),
            _ => Err("Expected an Option WitValue".to_string()),
        }
    }
}
