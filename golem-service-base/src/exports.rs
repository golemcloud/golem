use crate::model::{Export, ExportFunction, ExportInstance};
use rib::ParsedFunctionName;

pub fn instances(exports: &Vec<Export>) -> Vec<ExportInstance> {
    let mut instances = vec![];
    for export in exports {
        if let Export::Instance(instance) = export {
            instances.push(instance.clone())
        }
    }
    instances
}

pub fn functions(exports: &Vec<Export>) -> Vec<ExportFunction> {
    let mut functions = vec![];
    for export in exports {
        if let Export::Function(function) = export {
            functions.push(function.clone())
        }
    }
    functions
}

pub fn function_by_name(
    exports: &Vec<Export>,
    name: &str,
) -> Result<Option<ExportFunction>, String> {
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
