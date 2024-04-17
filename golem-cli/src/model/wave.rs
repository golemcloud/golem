use golem_client::model::{
    ExportFunction, ResourceMode, Type, TypeHandle, TypeRecord, TypeResult, TypeTuple, TypeVariant,
};
use golem_wasm_ast::analysis::{
    AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId,
    AnalysedResourceMode, AnalysedType,
};

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

pub fn func_to_analysed(func: &ExportFunction) -> AnalysedFunction {
    AnalysedFunction {
        name: func.name.clone(),
        params: func
            .parameters
            .iter()
            .map(|p| AnalysedFunctionParameter {
                name: p.name.to_string(),
                typ: type_to_analysed(&p.typ),
            })
            .collect(),
        results: func
            .results
            .iter()
            .map(|r| AnalysedFunctionResult {
                name: r.name.clone(),
                typ: type_to_analysed(&r.typ),
            })
            .collect(),
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
