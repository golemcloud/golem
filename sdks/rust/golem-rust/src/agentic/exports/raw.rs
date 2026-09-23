// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Dispatch before lifting canonical arguments, so absent capabilities do not
//! retain their resource-bearing lift, lower, and post-return implementations.
//! Only implementation constructors install the typed canonical entry points.

use super::{agent, load, middleware, save, tool};
use std::sync::OnceLock;

// Keep the full generated component-type sections. The world export macros
// embed them, but their interface macros must not also emit typed entry points.
macro_rules! ignore_exports {
    ($ty:ident with_types_in $($path:tt)*) => {};
}
pub(crate) use ignore_exports;

macro_rules! metadata_only {
    ($($package:ident { $($interface:ident: $export:ident),+ }),+) => {
        mod metadata {
            pub(crate) use super::ignore_exports;
            pub mod exports {
                pub mod golem {
                    $(pub mod $package {
                        $(pub mod $interface {
                            pub(crate) use super::super::super::super::ignore_exports as $export;
                        })+
                    })+
                }
            }
        }
    };
}

metadata_only!(
    agent {
        guest: __export_golem_agent_guest_2_0_0_cabi
    },
    tool {
        guest: __export_golem_tool_guest_0_1_0_cabi,
        tool_middleware_guest: __export_golem_tool_tool_middleware_guest_0_1_0_cabi
    },
    api {
        save_snapshot: __export_golem_api_save_snapshot_1_5_0_cabi,
        load_snapshot: __export_golem_api_load_snapshot_1_5_0_cabi
    }
);

crate::golem_agentic::export_golem_agentic!(Component with_types_in metadata);
crate::save_snapshot::export_save_snapshot!(Component with_types_in metadata);
crate::load_snapshot::export_load_snapshot!(Component with_types_in metadata);

// Signatures and layouts follow the pinned wit-bindgen's generated export
// macros. Sync results use a returned pointer and a separate post-return;
// async exports return the task status and complete through task-return imports.
macro_rules! capability {
    ($module:ident, [$($bound:path),+], {
        $( $name:ident ($($arg:ident: $ty:ty),*) -> $result:ty =
            $export:literal, $typed:expr, $absent:expr; )+
    }) => {
        pub(crate) mod $module {
            use super::*;

            struct Exports {
                $( $name: unsafe fn($($ty),*) -> $result, )+
            }

            static EXPORTS: OnceLock<Exports> = OnceLock::new();

            pub(crate) fn install<T: Sized $(+ $bound)+>() {
                let _ = EXPORTS.set(Exports { $( $name: $typed, )+ });
            }

            $(
                #[unsafe(export_name = $export)]
                unsafe extern "C" fn $name($($arg: $ty),*) -> $result {
                    wit_bindgen::rt::run_ctors_once();
                    match EXPORTS.get() {
                        Some(exports) => unsafe { (exports.$name)($($arg),*) },
                        None => unsafe { ($absent)($($arg),*) },
                    }
                }
            )+
        }
    };
}

#[repr(align(8))]
struct ReturnArea([usize; 4]);

static mut EMPTY: ReturnArea = ReturnArea([0; 4]);
static mut NOT_FOUND: ReturnArea = ReturnArea([0; 4]);

unsafe fn empty() -> *mut u8 {
    (&raw mut EMPTY).cast()
}

unsafe fn post_empty(_: *mut u8) {}

// Transfer ownership of the lowered name into InvalidToolName without lifting
// it into a Rust String. Only post-return releases it, including non-ASCII names.
unsafe fn not_found(name: *mut u8, len: usize) -> *mut u8 {
    unsafe { NOT_FOUND.0 = [1, 0, name as usize, len] };
    (&raw mut NOT_FOUND).cast()
}

unsafe fn post_not_found(result: *mut u8) {
    unsafe {
        let words = result.cast::<usize>();
        let len = *words.add(3);
        if len != 0 {
            std::alloc::dealloc(
                *words.add(2) as *mut u8,
                std::alloc::Layout::from_size_align_unchecked(len, 1),
            );
        }
    }
}

// Discovery returns no implementations. The host cannot select an absent
// agent/tool/middleware for invocation; trapping also avoids taking ownership
// of resource handles from an invocation which cannot be executed.
unsafe fn absent_invoke(_: *mut u8) -> i32 {
    core::arch::wasm32::unreachable()
}

unsafe fn absent_callback(_: u32, _: u32, _: u32) -> u32 {
    core::arch::wasm32::unreachable()
}

unsafe fn absent_definition() -> *mut u8 {
    core::arch::wasm32::unreachable()
}

unsafe fn absent_save() -> i32 {
    core::arch::wasm32::unreachable()
}

// Snapshot loading can be requested independently of discovery. Release the
// two canonical buffers, resolve through task-return, then report EXIT (0).
unsafe fn absent_load(payload: *mut u8, len: usize, mime: *mut u8, mime_len: usize) -> i32 {
    #[link(wasm_import_module = "[export]golem:api/load-snapshot@1.5.0")]
    unsafe extern "C" {
        #[link_name = "[task-return]load"]
        fn task_return(tag: i32, error: *const u8, len: usize);
    }
    for (ptr, len) in [(payload, len), (mime, mime_len)] {
        if len != 0 {
            unsafe {
                std::alloc::dealloc(ptr, std::alloc::Layout::from_size_align_unchecked(len, 1));
            }
        }
    }
    let error = "component has no agent snapshot support";
    unsafe {
        task_return(1, error.as_ptr(), error.len());
    }
    0
}

capability!(agent_exports, [agent::Guest, load::Guest, save::Guest], {
    initialize(p: *mut u8) -> i32 = "[async-lift]golem:agent/guest@2.0.0#initialize", agent::_export_initialize_cabi::<T>, absent_invoke;
    initialize_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:agent/guest@2.0.0#initialize", agent::__callback_initialize, absent_callback;
    invoke(p: *mut u8) -> i32 = "[async-lift]golem:agent/guest@2.0.0#invoke", agent::_export_invoke_cabi::<T>, absent_invoke;
    invoke_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:agent/guest@2.0.0#invoke", agent::__callback_invoke, absent_callback;
    definition() -> *mut u8 = "golem:agent/guest@2.0.0#get-definition", agent::_export_get_definition_cabi::<T>, absent_definition;
    definition_post(p: *mut u8) -> () = "cabi_post_golem:agent/guest@2.0.0#get-definition", agent::__post_return_get_definition::<T>, post_empty;
    discover() -> *mut u8 = "golem:agent/guest@2.0.0#discover-agent-types", agent::_export_discover_agent_types_cabi::<T>, empty;
    discover_post(p: *mut u8) -> () = "cabi_post_golem:agent/guest@2.0.0#discover-agent-types", agent::__post_return_discover_agent_types::<T>, post_empty;
    save() -> i32 = "[async-lift]golem:api/save-snapshot@1.5.0#save", save::_export_save_cabi::<T>, absent_save;
    save_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:api/save-snapshot@1.5.0#save", save::__callback_save, absent_callback;
    load(p: *mut u8, n: usize, m: *mut u8, l: usize) -> i32 = "[async-lift]golem:api/load-snapshot@1.5.0#load", load::_export_load_cabi::<T>, absent_load;
    load_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:api/load-snapshot@1.5.0#load", load::__callback_load, absent_callback;
});

capability!(tool_exports, [tool::Guest], {
    discover() -> *mut u8 = "golem:tool/guest@0.1.0#discover-tools", tool::_export_discover_tools_cabi::<T>, empty;
    discover_post(p: *mut u8) -> () = "cabi_post_golem:tool/guest@0.1.0#discover-tools", tool::__post_return_discover_tools::<T>, post_empty;
    get(p: *mut u8, n: usize) -> *mut u8 = "golem:tool/guest@0.1.0#get-tool", tool::_export_get_tool_cabi::<T>, not_found;
    get_post(p: *mut u8) -> () = "cabi_post_golem:tool/guest@0.1.0#get-tool", tool::__post_return_get_tool::<T>, post_not_found;
    invoke(p: *mut u8) -> i32 = "[async-lift]golem:tool/guest@0.1.0#invoke", tool::_export_invoke_cabi::<T>, absent_invoke;
    invoke_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:tool/guest@0.1.0#invoke", tool::__callback_invoke, absent_callback;
});

capability!(middleware_exports, [middleware::Guest], {
    discover() -> *mut u8 = "golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", middleware::_export_discover_tool_middlewares_cabi::<T>, empty;
    discover_post(p: *mut u8) -> () = "cabi_post_golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", middleware::__post_return_discover_tool_middlewares::<T>, post_empty;
    get(p: *mut u8, n: usize) -> *mut u8 = "golem:tool/tool-middleware-guest@0.1.0#get-tool-middleware", middleware::_export_get_tool_middleware_cabi::<T>, not_found;
    get_post(p: *mut u8) -> () = "cabi_post_golem:tool/tool-middleware-guest@0.1.0#get-tool-middleware", middleware::__post_return_get_tool_middleware::<T>, post_not_found;
    invoke(p: *mut u8) -> i32 = "[async-lift]golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware", middleware::_export_invoke_tool_middleware_cabi::<T>, absent_invoke;
    invoke_callback(a: u32, b: u32, c: u32) -> u32 = "[callback][async-lift]golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware", middleware::__callback_invoke_tool_middleware, absent_callback;
});
