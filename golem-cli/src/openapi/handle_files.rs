use std::collections::BTreeSet;

use anyhow::{anyhow, Error};
use tailcall_valid::{Valid, Validator};

use crate::wit_config::config::{WitConfig, Field, Interface, Record};
use crate::wit_config::wit_types::WitType;
use crate::openapi::openapi_spec::{OpenApiSpec, Resolved};

pub fn handle_types(mut config: WitConfig, spec: &OpenApiSpec<Resolved>) -> Valid<WitConfig, Error, Error> {
    let mut generated_records = BTreeSet::new();

    fn process_wit_type(
        wit_type: WitType,
        parent_name: &str,
        field_name: &str,
        generated_records: &mut BTreeSet<Record>,
    ) -> WitType {
        match wit_type {
            WitType::Record(fields) => {
                let record_name = format!("{}-{}", parent_name, field_name);
                let new_record_fields: BTreeSet<_> = fields
                    .into_iter()
                    .map(|(nested_field_name, nested_type)| {
                        let processed_type =
                            process_wit_type(nested_type, &record_name, &nested_field_name, generated_records);
                        Field {
                            name: nested_field_name,
                            field_type: processed_type,
                        }
                    })
                    .collect();

                let new_record = Record {
                    name: record_name.clone(),
                    fields: new_record_fields,
                    added_fields: Default::default(),
                };

                generated_records.insert(new_record);
                WitType::FieldTy(record_name)
            }
            WitType::Option(inner) => WitType::Option(Box::new(process_wit_type(*inner, parent_name, field_name, generated_records))),
            WitType::Result(ok, err) => WitType::Result(
                Box::new(process_wit_type(*ok, parent_name, field_name, generated_records)),
                Box::new(process_wit_type(*err, parent_name, field_name, generated_records)),
            ),
            WitType::List(inner) => WitType::List(Box::new(process_wit_type(*inner, parent_name, field_name, generated_records))),
            WitType::Tuple(elements) => WitType::Tuple(
                elements
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| process_wit_type(t, parent_name, &format!("{}_tuple_{}", field_name, i), generated_records))
                    .collect(),
            ),
            other => other,
        }
    }

    Valid::from_option(spec.components.as_ref(), anyhow!("Components are required"))
        .and_then(|components| Valid::from_option(components.schemas.as_ref(), anyhow!("Schemas are required")))
        .and_then(|schemas| {
            Valid::from_iter(schemas.iter(), |(record_name, schema)| {
                Valid::from_option(schema.type_.as_ref(), anyhow!("Type is required"))
                    .and_then(|_type_| {
                        Valid::from_option(schema.properties.as_ref(), anyhow!("Properties are required"))
                            .and_then(|properties| {
                                Valid::from_iter(properties, |(field_name, field_schema)| {
                                    Valid::from(WitType::from_schema(field_schema, spec))
                                        .and_then(|wit_type| {
                                            let processed_type = process_wit_type(
                                                wit_type,
                                                record_name,
                                                field_name,
                                                &mut generated_records,
                                            );
                                            Valid::succeed(Field {
                                                name: field_name.clone(),
                                                field_type: processed_type,
                                            })
                                        })
                                })
                            })
                            .and_then(|fields| {
                                let record = Record {
                                    name: record_name.clone(),
                                    fields: fields.into_iter().collect(),
                                    added_fields: Default::default(),
                                };
                                Valid::succeed(record)
                            })
                    })
            })
        })
        .and_then(|mut records| {
            records.extend(generated_records.into_iter());

            let interface = Interface {
                name: "types".to_string(),
                records: records.into_iter().collect(),
                ..Default::default()
            };
            config.interfaces.insert(interface);
            Valid::succeed(config)
        })
}
