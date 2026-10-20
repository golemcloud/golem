// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use golem_common::model::oplog::payload::{
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemToolTools, HostResponseRandomBytes,
};
use test_r::test;

#[test]
fn response_encoding_preserves_payload() {
    let response = HostResponseRandomBytes {
        bytes: vec![1, 2, 3, 4],
    };
    let expected: HostResponse = response.clone().into();
    let (returned, bytes) = *encode_response(Box::new(response.clone())).unwrap();
    assert_eq!(*returned, response);
    assert_eq!(
        bytes,
        golem_common::serialization::serialize(&expected).unwrap()
    );
    assert_eq!(
        golem_common::serialization::deserialize::<HostResponse>(&bytes).unwrap(),
        expected
    );
    assert!(response_has_bounded_encoding_stack::<HostResponseRandomBytes>());
    assert!(!response_has_bounded_encoding_stack::<
        HostResponseGolemRpcInvokeAndAwait,
    >());
    assert!(!response_has_bounded_encoding_stack::<
        HostResponseGolemToolTools,
    >());
}

#[test]
async fn response_encoding_on_wasmtime_fiber() -> anyhow::Result<()> {
    let config = golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache();
    let engine = wasmtime::Engine::new(&config)?;
    let module = wasmtime::Module::new(
        &engine,
        r#"(module
                (import "host" "encode" (func $encode))
                (func $nested (param $depth i32) (result i32)
                    local.get $depth
                    if (result i32)
                        local.get $depth
                        i32.const 1
                        i32.sub
                        call $nested
                        i32.const 1
                        i32.add
                    else
                        call $encode
                        i32.const 0
                    end)
                (func (export "run") (result i32)
                    i32.const 512
                    call $nested))"#,
    )?;
    let mut linker = wasmtime::Linker::new(&engine);
    linker.func_wrap("host", "encode", || -> wasmtime::Result<()> {
        for length in [0, 4, 64 * 1024] {
            let response = HostResponseRandomBytes {
                bytes: vec![42; length],
            };
            let (returned, bytes) = *encode_response(Box::new(response))
                .map_err(wasmtime::Error::msg)?;
            assert_eq!(returned.bytes.len(), length);
            assert!(!bytes.is_empty());
            assert!(matches!(golem_common::serialization::deserialize::<HostResponse>(&bytes).unwrap(), HostResponse::RandomBytes(value) if value.bytes == returned.bytes));
        }
        Ok(())
    })?;
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);
    let instance = linker.instantiate_async(&mut store, &module).await?;
    let run = instance.get_typed_func::<(), i32>(&mut store, "run")?;
    assert_eq!(run.call_async(&mut store, ()).await?, 512);
    Ok(())
}
