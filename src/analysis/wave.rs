use crate::analysis::{AnalysedFunction, AnalysedType, TypeEnum, TypeFlags, TypeList, TypeOption, TypeRecord, TypeResult, TypeTuple, TypeVariant};
use std::borrow::Cow;
use std::fmt::Display;
use wasm_wave::wasm::{DisplayType, WasmFunc, WasmType, WasmTypeKind};

impl WasmType for AnalysedType {
    fn kind(&self) -> WasmTypeKind {
        match self {
            AnalysedType::Bool(_) => WasmTypeKind::Bool,
            AnalysedType::S8(_) => WasmTypeKind::S8,
            AnalysedType::U8(_) => WasmTypeKind::U8,
            AnalysedType::S16(_) => WasmTypeKind::S16,
            AnalysedType::U16(_) => WasmTypeKind::U16,
            AnalysedType::S32(_) => WasmTypeKind::S32,
            AnalysedType::U32(_) => WasmTypeKind::U32,
            AnalysedType::S64(_) => WasmTypeKind::S64,
            AnalysedType::U64(_) => WasmTypeKind::U64,
            AnalysedType::F32(_) => WasmTypeKind::Float32,
            AnalysedType::F64(_) => WasmTypeKind::Float64,
            AnalysedType::Chr(_) => WasmTypeKind::Char,
            AnalysedType::Str(_) => WasmTypeKind::String,
            AnalysedType::List(_) => WasmTypeKind::List,
            AnalysedType::Tuple(_) => WasmTypeKind::Tuple,
            AnalysedType::Record(_) => WasmTypeKind::Record,
            AnalysedType::Flags(_) => WasmTypeKind::Flags,
            AnalysedType::Enum(_) => WasmTypeKind::Enum,
            AnalysedType::Option(_) => WasmTypeKind::Option,
            AnalysedType::Result { .. } => WasmTypeKind::Result,
            AnalysedType::Variant(_) => WasmTypeKind::Variant,
            AnalysedType::Handle(_) => WasmTypeKind::Unsupported,
        }
    }

    fn list_element_type(&self) -> Option<Self> {
        if let AnalysedType::List(TypeList { inner: ty }) = self {
            Some(*ty.clone())
        } else {
            None
        }
    }

    fn record_fields(&self) -> Box<dyn Iterator<Item=(Cow<str>, Self)> + '_> {
        if let AnalysedType::Record(TypeRecord { fields }) = self {
            Box::new(
                fields
                    .iter()
                    .map(|pair| (Cow::Borrowed(pair.name.as_str()), pair.typ.clone())),
            )
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn tuple_element_types(&self) -> Box<dyn Iterator<Item=Self> + '_> {
        if let AnalysedType::Tuple(TypeTuple { items }) = self {
            Box::new(items.clone().into_iter())
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn variant_cases(&self) -> Box<dyn Iterator<Item=(Cow<str>, Option<Self>)> + '_> {
        if let AnalysedType::Variant(TypeVariant { cases }) = self {
            Box::new(
                cases
                    .iter()
                    .map(|case| (Cow::Borrowed(case.name.as_str()), case.typ.clone())),
            )
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn enum_cases(&self) -> Box<dyn Iterator<Item=Cow<str>> + '_> {
        if let AnalysedType::Enum(TypeEnum { cases }) = self {
            Box::new(cases.iter().map(|name| Cow::Borrowed(name.as_str())))
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn option_some_type(&self) -> Option<Self> {
        if let AnalysedType::Option(TypeOption { inner }) = self {
            Some(*inner.clone())
        } else {
            None
        }
    }

    fn result_types(&self) -> Option<(Option<Self>, Option<Self>)> {
        if let AnalysedType::Result(TypeResult { ok, err }) = self {
            Some((
                ok.as_ref().map(|t| *t.clone()),
                err.as_ref().map(|t| *t.clone()),
            ))
        } else {
            None
        }
    }

    fn flags_names(&self) -> Box<dyn Iterator<Item=Cow<str>> + '_> {
        if let AnalysedType::Flags(TypeFlags { names }) = self {
            Box::new(names.iter().map(|name| Cow::Borrowed(name.as_str())))
        } else {
            Box::new(std::iter::empty())
        }
    }
}

impl WasmFunc for AnalysedFunction {
    type Type = AnalysedType;

    fn params(&self) -> Box<dyn Iterator<Item=Self::Type> + '_> {
        Box::new(self.params.iter().map(|p| p.typ.clone()))
    }

    fn param_names(&self) -> Box<dyn Iterator<Item=Cow<str>> + '_> {
        Box::new(self.params.iter().map(|p| Cow::Borrowed(p.name.as_str())))
    }

    fn results(&self) -> Box<dyn Iterator<Item=Self::Type> + '_> {
        Box::new(self.results.iter().map(|r| r.typ.clone()))
    }

    fn result_names(&self) -> Box<dyn Iterator<Item=Cow<str>> + '_> {
        let names: Option<Vec<Cow<str>>> = self
            .results
            .iter()
            .map(|r| r.name.as_ref().map(|n| Cow::Borrowed(n.as_str())))
            .collect();

        match names {
            Some(names) => Box::new(names.into_iter()),
            None => Box::new(std::iter::empty()),
        }
    }
}

/// Copy of DisplayFunc with additional name filed.
/// DisplayFunc is always using func for name
pub struct DisplayNamedFunc<T: WasmFunc> {
    pub name: String,
    pub func: T,
}

impl<T: WasmFunc> Display for DisplayNamedFunc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        f.write_str("(")?;
        let mut param_names = self.func.param_names();
        for (idx, ty) in self.func.params().enumerate() {
            if idx != 0 {
                f.write_str(", ")?;
            }
            if let Some(name) = param_names.next() {
                write!(f, "{name}: ")?;
            }
            DisplayType(&ty).fmt(f)?
        }
        f.write_str(")")?;

        let results = self.func.results().collect::<Vec<_>>();
        if results.is_empty() {
            return Ok(());
        }

        let mut result_names = self.func.result_names();
        if results.len() == 1 {
            let ty = DisplayType(&results.into_iter().next().unwrap()).to_string();
            if let Some(name) = result_names.next() {
                write!(f, " -> ({name}: {ty})")
            } else {
                write!(f, " -> {ty}")
            }
        } else {
            f.write_str(" -> (")?;
            for (idx, ty) in results.into_iter().enumerate() {
                if idx != 0 {
                    f.write_str(", ")?;
                }
                if let Some(name) = result_names.next() {
                    write!(f, "{name}: ")?;
                }
                DisplayType(&ty).fmt(f)?;
            }
            f.write_str(")")
        }
    }
}
