use golem_wasm_rpc_stubgen::WasmRpcOverride;
use std::path::{Path, PathBuf};

pub static TEST_DATA_PATH: &str = "../test-data";
pub static WASM_RPC_PATH: &str = "../../wasm-rpc";

pub fn test_data_path() -> &'static Path {
    Path::new(TEST_DATA_PATH)
}

pub fn wasm_rpc_path() -> PathBuf {
    std::env::current_dir()
        .expect("Failed to get current dir")
        .join(WASM_RPC_PATH)
}

pub fn wasm_rpc_path_override() -> Option<String> {
    Some(wasm_rpc_path().to_string_lossy().to_string())
}

pub fn wasm_rpc_override() -> WasmRpcOverride {
    WasmRpcOverride {
        wasm_rpc_path_override: wasm_rpc_path_override(),
        wasm_rpc_version_override: None,
    }
}
