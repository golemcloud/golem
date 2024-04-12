use golem_client::model::{
    ExportFunction, ResourceMode, Type, TypeHandle, TypeRecord, TypeResult, TypeTuple, TypeVariant,
};
use golem_wasm_ast::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType};
use std::borrow::Cow;
use std::fmt::Display;
use wasm_wave::wasm::{DisplayType, WasmFunc};

pub fn type_wave_compatible(typ: &Type) -> bool {
    fn variant_wave_compatible(tv: &TypeVariant) -> bool {
        tv.cases
            .iter()
            .all(|notp| notp.typ.iter().all(type_wave_compatible))
    }

    fn result_wave_compatible(tr: &TypeResult) -> bool {
        tr.ok.iter().all(type_wave_compatible) && tr.err.iter().all(type_wave_compatible)
    }

    fn record_wave_compatible(tr: &TypeRecord) -> bool {
        tr.cases.iter().all(|ntp| type_wave_compatible(&ntp.typ))
    }

    fn tuple_wave_compatible(tt: &TypeTuple) -> bool {
        tt.items.iter().all(type_wave_compatible)
    }

    match typ {
        Type::Variant(tv) => variant_wave_compatible(tv),
        Type::Result(tr) => result_wave_compatible(tr),
        Type::Option(to) => type_wave_compatible(&to.inner),
        Type::Enum(_) => true,
        Type::Flags(_) => true,
        Type::Record(tr) => record_wave_compatible(tr),
        Type::Tuple(tt) => tuple_wave_compatible(tt),
        Type::List(tl) => type_wave_compatible(&tl.inner),
        Type::Str(_) => true,
        Type::Chr(_) => true,
        Type::F64(_) => true,
        Type::F32(_) => true,
        Type::U64(_) => true,
        Type::S64(_) => true,
        Type::U32(_) => true,
        Type::S32(_) => true,
        Type::U16(_) => true,
        Type::S16(_) => true,
        Type::U8(_) => true,
        Type::S8(_) => true,
        Type::Bool(_) => true,
        Type::Handle(_) => false,
    }
}

pub fn function_wave_compatible(func: &ExportFunction) -> bool {
    func.parameters.iter().all(|p| type_wave_compatible(&p.typ))
        && func.results.iter().all(|r| type_wave_compatible(&r.typ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapExportFunction(pub ExportFunction);

fn wrap_analysed_type(typ: AnalysedType) -> golem_wasm_rpc::AnalysedType {
    golem_wasm_rpc::AnalysedType(typ)
}

pub fn wrap_type(typ: &Type) -> golem_wasm_rpc::AnalysedType {
    wrap_analysed_type(type_to_analysed(typ))
}

impl WasmFunc for WrapExportFunction {
    type Type = golem_wasm_rpc::AnalysedType;

    fn params(&self) -> Box<dyn Iterator<Item = Self::Type> + '_> {
        Box::new(self.0.parameters.iter().map(|p| wrap_type(&p.typ)))
    }

    fn param_names(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        Box::new(
            self.0
                .parameters
                .iter()
                .map(|p| Cow::Borrowed(p.name.as_str())),
        )
    }

    fn results(&self) -> Box<dyn Iterator<Item = Self::Type> + '_> {
        Box::new(self.0.results.iter().map(|r| wrap_type(&r.typ)))
    }

    fn result_names(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        let names: Option<Vec<Cow<str>>> = self
            .0
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

pub fn type_to_analysed(typ: &Type) -> AnalysedType {
    fn variant_to_analysed(tv: &TypeVariant) -> AnalysedType {
        AnalysedType::Variant(
            tv.cases
                .iter()
                .map(|notp| (notp.name.clone(), notp.typ.as_ref().map(type_to_analysed)))
                .collect(),
        )
    }

    fn result_to_analysed(tr: &TypeResult) -> AnalysedType {
        AnalysedType::Result {
            ok: tr.ok.as_ref().map(|t| Box::new(type_to_analysed(t))),
            error: tr.err.as_ref().map(|t| Box::new(type_to_analysed(t))),
        }
    }

    fn record_to_analysed(tr: &TypeRecord) -> AnalysedType {
        AnalysedType::Record(
            tr.cases
                .iter()
                .map(|ntp| (ntp.name.clone(), type_to_analysed(&ntp.typ)))
                .collect(),
        )
    }

    fn handle_to_analysed(th: &TypeHandle) -> AnalysedType {
        AnalysedType::Resource {
            id: AnalysedResourceId {
                value: th.resource_id,
            },
            resource_mode: match th.mode {
                ResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
                ResourceMode::Owned => AnalysedResourceMode::Owned,
            },
        }
    }

    match typ {
        Type::Variant(tv) => variant_to_analysed(tv),
        Type::Result(tr) => result_to_analysed(tr),
        Type::Option(to) => AnalysedType::Option(Box::new(type_to_analysed(&to.inner))),
        Type::Enum(te) => AnalysedType::Enum(te.cases.clone()),
        Type::Flags(tf) => AnalysedType::Flags(tf.cases.clone()),
        Type::Record(tr) => record_to_analysed(tr),
        Type::Tuple(tt) => AnalysedType::Tuple(tt.items.iter().map(type_to_analysed).collect()),
        Type::List(tl) => AnalysedType::List(Box::new(type_to_analysed(&tl.inner))),
        Type::Str(_) => AnalysedType::Str,
        Type::Chr(_) => AnalysedType::Chr,
        Type::F64(_) => AnalysedType::F64,
        Type::F32(_) => AnalysedType::F32,
        Type::U64(_) => AnalysedType::U64,
        Type::S64(_) => AnalysedType::S64,
        Type::U32(_) => AnalysedType::U32,
        Type::S32(_) => AnalysedType::S32,
        Type::U16(_) => AnalysedType::U16,
        Type::S16(_) => AnalysedType::S16,
        Type::U8(_) => AnalysedType::U8,
        Type::S8(_) => AnalysedType::S8,
        Type::Bool(_) => AnalysedType::Bool,
        Type::Handle(th) => handle_to_analysed(th),
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
