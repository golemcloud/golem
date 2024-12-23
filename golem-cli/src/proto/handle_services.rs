use convert_case::Case;
use protox::prost_reflect::prost_types::{FileDescriptorSet, ServiceDescriptorProto};
use tailcall_valid::{Valid, Validator};
use crate::wit_config::config::{WitConfig, Function, Interface, Parameter, ReturnTy, UseStatement};
use convert_case::Casing;
use crate::proto::proto::process_ty;

fn handle_service(config: &WitConfig, services: &[ServiceDescriptorProto]) -> Valid<Vec<Interface>, anyhow::Error, anyhow::Error> {
    Valid::from_iter(services.iter(), |service| {
        let name = service.name().to_case(Case::Kebab);
        Valid::from_iter(service.method.iter(), |method| {
            let name = method.name().to_case(Case::Kebab);
            let input_ty = if let Some(input) = method.input_type.as_ref() {
                process_ty(input).map(|v| Some(v))
            } else {
                Valid::succeed(None)
            };

            let output_ty = if let Some(output) = method.output_type.as_ref() {
                process_ty(output).map(|v| Some(v))
            } else {
                Valid::succeed(None)
            };

            input_ty.zip(output_ty).map(|(a, b)| {
                let mut parameters = vec![];
                let mut return_type = ReturnTy {
                    return_type: "unit".to_string(),

                    // TODO: I am not yet sure about error type
                    error_type: None,
                };
                if let Some(a) = a {
                    // Protobuf only supports one input parameter,
                    // so we can assume that the name is "input"
                    parameters.push(Parameter {
                        name: "input".to_string(),
                        parameter_type: a.to_wit(None),
                    });
                }
                if let Some(b) = b {
                    return_type.return_type = b.to_wit(None);
                }
                Function {
                    name,
                    parameters,
                    return_type,
                }
            })
        }).and_then(|functions| {
            Valid::from_iter(config.interfaces.iter(), |interface| {
                let use_name = interface.name.to_string();
                let mut imports = vec![];
                imports.extend(interface.records.iter().map(|v| v.name.to_string()));
                imports.extend(interface.varients.iter().map(|(k, _)| k.to_string()));

                Valid::succeed(
                    UseStatement {
                        name: use_name,
                        items: imports,
                    }
                )
            }).and_then(|uses| {
                Valid::succeed(Interface {
                    name,
                    uses,
                    functions,
                    ..Default::default()
                })
            })
        })
    })
}

pub fn handle_services(config: WitConfig, proto: &[FileDescriptorSet]) -> Valid<WitConfig, anyhow::Error, anyhow::Error> {
    Valid::succeed(config)
        .and_then(|mut config| {
            Valid::from_iter(proto.iter(), |file| {
                Valid::from_iter(file.file.iter(), |file| {
                    handle_service(&config, &file.service)
                })
                    .and_then(|interfaces| {
                        config.interfaces.extend(interfaces.into_iter().flatten().collect::<Vec<_>>());
                        Valid::succeed(())
                    })
            }).and_then(|_| Valid::succeed(config))
        })
}