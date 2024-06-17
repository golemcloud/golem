#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
impl crate::bindings::exports::golem::it_stub::stub_rust_component_service::GuestApi
for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn echo(&self, input: String) -> String {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{echo}",
                &[WitValue::builder().string(&input)],
            )
            .expect(
                &format!("Failed to invoke-and-await remote {}", "golem:it/api.{echo}"),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .string()
            .expect("string not found")
            .to_string())
    }
    fn calculate(&self, input: u64) -> u64 {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{calculate}",
                &[WitValue::builder().u64(input)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}", "golem:it/api.{calculate}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
    fn process(
        &self,
        input: Vec<crate::bindings::golem::it::api::Data>,
    ) -> Vec<crate::bindings::golem::it::api::Data> {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{process}",
                &[
                    WitValue::builder()
                        .list_fn(
                            &input,
                            |item, item_builder| {
                                item_builder
                                    .record()
                                    .item()
                                    .string(&item.id)
                                    .item()
                                    .string(&item.name)
                                    .item()
                                    .string(&item.desc)
                                    .item()
                                    .u64(item.timestamp)
                                    .finish()
                            },
                        ),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}", "golem:it/api.{process}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| {
                let record = item;
                crate::bindings::golem::it::api::Data {
                    id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    name: record
                        .field(1usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    desc: record
                        .field(2usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    timestamp: record
                        .field(3usize)
                        .expect("record field not found")
                        .u64()
                        .expect("u64 not found"),
                }
            })
            .expect("list not found"))
    }
}
