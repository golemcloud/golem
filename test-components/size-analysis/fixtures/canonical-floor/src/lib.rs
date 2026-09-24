use std::alloc::{Layout, alloc, dealloc, realloc};

mod agentic {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-agentic",
        async: [
            "export:golem:agent/guest@2.0.0#initialize",
            "export:golem:agent/guest@2.0.0#invoke",
            "export:golem:tool/guest@0.1.0#invoke",
            "export:golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware",
        ],
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
    });

    pub use __export_golem_agentic_impl as export_golem_agentic;
}

mod save_snapshot {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-rust-save-snapshot",
        async: ["export:golem:api/save-snapshot@1.5.0#save"],
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
    });

    pub use __export_golem_rust_save_snapshot_impl as export_save_snapshot;
}

mod load_snapshot {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-rust-load-snapshot",
        async: ["export:golem:api/load-snapshot@1.5.0#load"],
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
    });

    pub use __export_golem_rust_load_snapshot_impl as export_load_snapshot;
}

macro_rules! ignore_exports {
    ($ty:ident with_types_in $($path:tt)*) => {};
}
pub(crate) use ignore_exports;

mod metadata {
    pub mod exports {
        pub mod golem {
            pub mod agent {
                pub mod guest {
                    pub(crate) use crate::ignore_exports as __export_golem_agent_guest_2_0_0_cabi;
                }
            }
            pub mod tool {
                pub mod guest {
                    pub(crate) use crate::ignore_exports as __export_golem_tool_guest_0_1_0_cabi;
                }
                pub mod tool_middleware_guest {
                    pub(crate) use crate::ignore_exports as __export_golem_tool_tool_middleware_guest_0_1_0_cabi;
                }
            }
            pub mod api {
                pub mod save_snapshot {
                    pub(crate) use crate::ignore_exports as __export_golem_api_save_snapshot_1_5_0_cabi;
                }
                pub mod load_snapshot {
                    pub(crate) use crate::ignore_exports as __export_golem_api_load_snapshot_1_5_0_cabi;
                }
            }
        }
    }
}

#[allow(dead_code)]
struct Component;
agentic::export_golem_agentic!(Component with_types_in metadata);
save_snapshot::export_save_snapshot!(Component with_types_in metadata);
load_snapshot::export_load_snapshot!(Component with_types_in metadata);

#[repr(align(8))]
struct ReturnArea([usize; 4]);

static mut EMPTY: ReturnArea = ReturnArea([0; 4]);
static mut NOT_FOUND: ReturnArea = ReturnArea([0; 4]);

#[unsafe(export_name = "cabi_realloc")]
unsafe extern "C" fn cabi_realloc(
    pointer: *mut u8,
    old_size: usize,
    alignment: usize,
    new_size: usize,
) -> *mut u8 {
    let layout = unsafe { Layout::from_size_align_unchecked(old_size, alignment) };
    if new_size == 0 {
        if old_size != 0 {
            unsafe { dealloc(pointer, layout) };
        }
        std::ptr::null_mut()
    } else if old_size == 0 {
        unsafe { alloc(Layout::from_size_align_unchecked(new_size, alignment)) }
    } else {
        unsafe { realloc(pointer, layout, new_size) }
    }
}

unsafe fn empty() -> *mut u8 {
    (&raw mut EMPTY).cast()
}

unsafe fn not_found(name: *mut u8, len: usize) -> *mut u8 {
    unsafe { NOT_FOUND.0 = [1, 0, name as usize, len] };
    (&raw mut NOT_FOUND).cast()
}

unsafe fn post_not_found(result: *mut u8) {
    let words = result.cast::<usize>();
    let len = unsafe { *words.add(3) };
    if len != 0 {
        unsafe {
            dealloc(
                *words.add(2) as *mut u8,
                Layout::from_size_align_unchecked(len, 1),
            );
        }
    }
}

macro_rules! export {
    ($name:ident($($arg:ident: $ty:ty),*) -> $result:ty, $export:literal, $body:expr) => {
        #[unsafe(export_name = $export)]
        unsafe extern "C" fn $name($($arg: $ty),*) -> $result {
            unsafe { ($body)($($arg),*) }
        }
    };
}

unsafe fn trap(_: *mut u8) -> i32 {
    core::arch::wasm32::unreachable()
}
unsafe fn callback(_: u32, _: u32, _: u32) -> u32 {
    core::arch::wasm32::unreachable()
}
unsafe fn trap_result() -> *mut u8 {
    core::arch::wasm32::unreachable()
}
unsafe fn trap_save() -> i32 {
    core::arch::wasm32::unreachable()
}
unsafe fn post_empty(_: *mut u8) {}

unsafe fn load(payload: *mut u8, len: usize, mime: *mut u8, mime_len: usize) -> i32 {
    #[link(wasm_import_module = "[export]golem:api/load-snapshot@1.5.0")]
    unsafe extern "C" {
        #[link_name = "[task-return]load"]
        fn task_return(tag: i32, error: *const u8, len: usize);
    }
    for (pointer, len) in [(payload, len), (mime, mime_len)] {
        if len != 0 {
            unsafe { dealloc(pointer, Layout::from_size_align_unchecked(len, 1)) };
        }
    }
    let error = "component has no agent snapshot support";
    unsafe { task_return(1, error.as_ptr(), error.len()) };
    0
}

export!(agent_initialize(p: *mut u8) -> i32, "[async-lift]golem:agent/guest@2.0.0#initialize", trap);
export!(agent_initialize_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:agent/guest@2.0.0#initialize", callback);
export!(agent_invoke(p: *mut u8) -> i32, "[async-lift]golem:agent/guest@2.0.0#invoke", trap);
export!(agent_invoke_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:agent/guest@2.0.0#invoke", callback);
export!(agent_definition() -> *mut u8, "golem:agent/guest@2.0.0#get-definition", trap_result);
export!(agent_definition_post(p: *mut u8) -> (), "cabi_post_golem:agent/guest@2.0.0#get-definition", post_empty);
export!(agent_discover() -> *mut u8, "golem:agent/guest@2.0.0#discover-agent-types", empty);
export!(agent_discover_post(p: *mut u8) -> (), "cabi_post_golem:agent/guest@2.0.0#discover-agent-types", post_empty);
export!(save() -> i32, "[async-lift]golem:api/save-snapshot@1.5.0#save", trap_save);
export!(save_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:api/save-snapshot@1.5.0#save", callback);
export!(load_snapshot(p: *mut u8, n: usize, m: *mut u8, l: usize) -> i32, "[async-lift]golem:api/load-snapshot@1.5.0#load", load);
export!(load_snapshot_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:api/load-snapshot@1.5.0#load", callback);
export!(tool_discover() -> *mut u8, "golem:tool/guest@0.1.0#discover-tools", empty);
export!(tool_discover_post(p: *mut u8) -> (), "cabi_post_golem:tool/guest@0.1.0#discover-tools", post_empty);
export!(tool_get(p: *mut u8, n: usize) -> *mut u8, "golem:tool/guest@0.1.0#get-tool", not_found);
export!(tool_get_post(p: *mut u8) -> (), "cabi_post_golem:tool/guest@0.1.0#get-tool", post_not_found);
export!(tool_invoke(p: *mut u8) -> i32, "[async-lift]golem:tool/guest@0.1.0#invoke", trap);
export!(tool_invoke_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:tool/guest@0.1.0#invoke", callback);
export!(middleware_discover() -> *mut u8, "golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", empty);
export!(middleware_discover_post(p: *mut u8) -> (), "cabi_post_golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", post_empty);
export!(middleware_get(p: *mut u8, n: usize) -> *mut u8, "golem:tool/tool-middleware-guest@0.1.0#get-tool-middleware", not_found);
export!(middleware_get_post(p: *mut u8) -> (), "cabi_post_golem:tool/tool-middleware-guest@0.1.0#get-tool-middleware", post_not_found);
export!(middleware_invoke(p: *mut u8) -> i32, "[async-lift]golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware", trap);
export!(middleware_invoke_callback(a: u32, b: u32, c: u32) -> u32, "[callback][async-lift]golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware", callback);
