use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedInstance,
};

use rib::ParsedFunctionName;

pub trait AnalysedExportExtensions {
    fn function_names(&self) -> Vec<String>;
}

impl AnalysedExportExtensions for AnalysedExport {
    fn function_names(&self) -> Vec<String> {
        match self {
            AnalysedExport::Instance(instance) => instance
                .functions
                .iter()
                .map(|function| format!("{}.{{{}}}", instance.name, function.name))
                .collect(),
            AnalysedExport::Function(function) => vec![function.name.clone()],
        }
    }
}

pub fn instances(exports: &Vec<AnalysedExport>) -> Vec<AnalysedInstance> {
    let mut instances = vec![];
    for export in exports {
        if let AnalysedExport::Instance(instance) = export {
            instances.push(instance.clone())
        }
    }
    instances
}

pub fn functions(exports: &Vec<AnalysedExport>) -> Vec<AnalysedFunction> {
    let mut functions = vec![];
    for export in exports {
        if let AnalysedExport::Function(function) = export {
            functions.push(function.clone())
        }
    }
    functions
}

pub fn function_by_name(
    exports: &Vec<AnalysedExport>,
    name: &str,
) -> Result<Option<AnalysedFunction>, String> {
    let parsed = ParsedFunctionName::parse(name)?;

    match &parsed.site().interface_name() {
        None => Ok(functions(exports).iter().find(|f| f.name == *name).cloned()),
        Some(interface_name) => {
            let exported_function = instances(exports)
                .iter()
                .find(|instance| instance.name == *interface_name)
                .and_then(|instance| {
                    instance
                        .functions
                        .iter()
                        .find(|f| f.name == parsed.function().function_name())
                        .cloned()
                });
            if exported_function.is_none() {
                match parsed.method_as_static() {
                    Some(parsed_static) => Ok(instances(exports)
                        .iter()
                        .find(|instance| instance.name == *interface_name)
                        .and_then(|instance| {
                            instance
                                .functions
                                .iter()
                                .find(|f| f.name == parsed_static.function().function_name())
                                .cloned()
                        })),
                    None => Ok(None),
                }
            } else {
                Ok(exported_function)
            }
        }
    }
}
