use std::collections::{BTreeMap, BTreeSet};

use convert_case::Case;
use convert_case::Casing;
use protox::prost_reflect::prost_types::{DescriptorProto, EnumDescriptorProto, FileDescriptorSet};
use tailcall_valid::{Valid, Validator};

use crate::wit_config::config::{WitConfig, Field, Interface, Record};
use crate::wit_config::wit_types::WitType;
use crate::proto::proto::process_ty;

fn append_enums(file: &[EnumDescriptorProto]) -> Valid<BTreeMap<String, WitType>, anyhow::Error, anyhow::Error> {
    Valid::from_iter(file.iter(), |enum_| {
        let enum_name = enum_.name().to_case(Case::Kebab);
        Valid::from_iter(enum_.value.iter(), |value| {
            Valid::succeed(value.name().to_case(Case::Kebab))
        }).and_then(|varients| {
            Valid::succeed((enum_name, WitType::Enum(varients)))
        })
    }).and_then(|rec| Valid::succeed(rec.into_iter().collect()))
}

fn append_message(messages: &[DescriptorProto]) -> Valid<Vec<Record>, anyhow::Error, anyhow::Error> {
    Valid::from_iter(messages.iter(), |message| {
        let record_name = message.name().to_case(Case::Kebab);
        Valid::from_iter(message.field.iter(), |field| {
            if let Some(ty_) = field.type_name.as_ref() {
                process_ty(ty_).map(|ty| (field.name().to_case(Case::Kebab), ty))
            } else {
                Valid::from(WitType::from_primitive_proto_type(field.r#type().as_str_name()))
                    .map(|ty| (field.name().to_case(Case::Kebab), ty))
            }.and_then(|(name, ty)| {
                Valid::succeed(Field {
                    name,
                    field_type: ty,
                })
            })
        }).and_then(|fields| {
            Valid::succeed(Record {
                name: record_name,
                fields: fields.into_iter().collect(),
                ..Default::default()
            })
        })
    })
}

pub fn handle_types(config: WitConfig, proto: &[FileDescriptorSet], package: String) -> Valid<WitConfig, anyhow::Error, anyhow::Error> {
    Valid::from_iter(proto.iter(), |set| {
        let mut map = BTreeMap::new();
        let mut records = BTreeSet::new();
        Valid::from_iter(set.file.iter(), |file| {
            append_enums(&file.enum_type).and_then(|varients| {
                append_message(&file.message_type)
                    .and_then(|recs| {
                        map.extend(varients);
                        records.extend(BTreeSet::from_iter(recs.into_iter()));
                        Valid::succeed(())
                    })
            })
        })
            .and_then(|_| {
                Valid::succeed(Interface {
                    name: "type".to_string(),
                    varients: map,
                    records,
                    ..Default::default()
                })
            })
    }).and_then(|mut interfaces| {
        interfaces.extend(config.interfaces.into_iter());
        Valid::succeed(WitConfig {
            package,
            interfaces: interfaces.into_iter().collect(),
            ..config
        })
    })
}
