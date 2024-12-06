#[allow(dead_code)]
pub mod golem {
    #[allow(dead_code)]
    pub mod api {
        #[allow(dead_code, clippy::all)]
        pub mod identity {
            #[used]
            #[doc(hidden)]
            static __FORCE_SECTION_REF: fn() = super::super::super::__link_custom_section_describing_imports;
            use super::super::super::_rt;
            #[allow(unused_unsafe, clippy::all)]
            /// Get identity token
            pub fn get_token() -> Option<_rt::String> {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 12]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 12]);
                    let ptr0 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(target_arch = "wasm32")]
                    #[link(wasm_import_module = "golem:api/identity@1.1.0")]
                    extern "C" {
                        #[link_name = "get-token"]
                        fn wit_import(_: *mut u8);
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: *mut u8) {
                        unreachable!()
                    }
                    wit_import(ptr0);
                    let l1 = i32::from(*ptr0.add(0).cast::<u8>());
                    match l1 {
                        0 => None,
                        1 => {
                            let e = {
                                let l2 = *ptr0.add(4).cast::<*mut u8>();
                                let l3 = *ptr0.add(8).cast::<usize>();
                                let len4 = l3;
                                let bytes4 = _rt::Vec::from_raw_parts(
                                    l2.cast(),
                                    len4,
                                    len4,
                                );
                                _rt::string_lift(bytes4)
                            };
                            Some(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
        }
    }
}
#[allow(dead_code)]
pub mod exports {
    #[allow(dead_code)]
    pub mod golem {
        #[allow(dead_code)]
        pub mod it {
            #[allow(dead_code, clippy::all)]
            pub mod api {
                #[used]
                #[doc(hidden)]
                static __FORCE_SECTION_REF: fn() = super::super::super::super::__link_custom_section_describing_imports;
                use super::super::super::super::_rt;
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_echo_cabi<T: Guest>(
                    arg0: *mut u8,
                    arg1: usize,
                ) -> *mut u8 {
                    #[cfg(target_arch = "wasm32")] _rt::run_ctors_once();
                    let len0 = arg1;
                    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
                    let result1 = T::echo(_rt::string_lift(bytes0));
                    let ptr2 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
                    let vec3 = (result1.into_bytes()).into_boxed_slice();
                    let ptr3 = vec3.as_ptr().cast::<u8>();
                    let len3 = vec3.len();
                    ::core::mem::forget(vec3);
                    *ptr2.add(4).cast::<usize>() = len3;
                    *ptr2.add(0).cast::<*mut u8>() = ptr3.cast_mut();
                    ptr2
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn __post_return_echo<T: Guest>(arg0: *mut u8) {
                    let l0 = *arg0.add(0).cast::<*mut u8>();
                    let l1 = *arg0.add(4).cast::<usize>();
                    _rt::cabi_dealloc(l0, l1, 1);
                }
                pub trait Guest {
                    fn echo(input: _rt::String) -> _rt::String;
                }
                #[doc(hidden)]
                macro_rules! __export_golem_it_api_cabi {
                    ($ty:ident with_types_in $($path_to_types:tt)*) => {
                        const _ : () = { #[export_name = "golem:it/api#echo"] unsafe
                        extern "C" fn export_echo(arg0 : * mut u8, arg1 : usize,) -> *
                        mut u8 { $($path_to_types)*:: _export_echo_cabi::<$ty > (arg0,
                        arg1) } #[export_name = "cabi_post_golem:it/api#echo"] unsafe
                        extern "C" fn _post_return_echo(arg0 : * mut u8,) {
                        $($path_to_types)*:: __post_return_echo::<$ty > (arg0) } };
                    };
                }
                #[doc(hidden)]
                pub(crate) use __export_golem_it_api_cabi;
                #[repr(align(4))]
                struct _RetArea([::core::mem::MaybeUninit<u8>; 8]);
                static mut _RET_AREA: _RetArea = _RetArea(
                    [::core::mem::MaybeUninit::uninit(); 8],
                );
            }
        }
    }
}
mod _rt {
    pub use alloc_crate::string::String;
    pub use alloc_crate::vec::Vec;
    pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
        if cfg!(debug_assertions) {
            String::from_utf8(bytes).unwrap()
        } else {
            String::from_utf8_unchecked(bytes)
        }
    }
    pub unsafe fn invalid_enum_discriminant<T>() -> T {
        if cfg!(debug_assertions) {
            panic!("invalid enum discriminant")
        } else {
            core::hint::unreachable_unchecked()
        }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn run_ctors_once() {
        wit_bindgen_rt::run_ctors_once();
    }
    pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
        if size == 0 {
            return;
        }
        let layout = alloc::Layout::from_size_align_unchecked(size, align);
        alloc::dealloc(ptr, layout);
    }
    extern crate alloc as alloc_crate;
    pub use alloc_crate::alloc;
}
/// Generates `#[no_mangle]` functions to export the specified type as the
/// root implementation of all generated traits.
///
/// For more information see the documentation of `wit_bindgen::generate!`.
///
/// ```rust
/// # macro_rules! export{ ($($t:tt)*) => (); }
/// # trait Guest {}
/// struct MyType;
///
/// impl Guest for MyType {
///     // ...
/// }
///
/// export!(MyType);
/// ```
#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! __export_rust_echo_impl {
    ($ty:ident) => {
        self::export!($ty with_types_in self);
    };
    ($ty:ident with_types_in $($path_to_types_root:tt)*) => {
        $($path_to_types_root)*::
        exports::golem::it::api::__export_golem_it_api_cabi!($ty with_types_in
        $($path_to_types_root)*:: exports::golem::it::api);
    };
}
#[doc(inline)]
pub(crate) use __export_rust_echo_impl as export;
#[cfg(target_arch = "wasm32")]
#[link_section = "component-type:wit-bindgen:0.35.0:golem:it:rust-echo:encoded world"]
#[doc(hidden)]
pub static __WIT_BINDGEN_COMPONENT_TYPE: [u8; 251] = *b"\
\0asm\x0d\0\x01\0\0\x19\x16wit-component-encoding\x04\0\x07|\x01A\x02\x01A\x04\x01\
B\x03\x01ks\x01@\0\0\0\x04\0\x09get-token\x01\x01\x03\0\x18golem:api/identity@1.\
1.0\x05\0\x01B\x02\x01@\x01\x05inputs\0s\x04\0\x04echo\x01\0\x04\0\x0cgolem:it/a\
pi\x05\x01\x04\0\x12golem:it/rust-echo\x04\0\x0b\x0f\x01\0\x09rust-echo\x03\0\0\0\
G\x09producers\x01\x0cprocessed-by\x02\x0dwit-component\x070.220.0\x10wit-bindge\
n-rust\x060.35.0";
#[inline(never)]
#[doc(hidden)]
pub fn __link_custom_section_describing_imports() {
    wit_bindgen_rt::maybe_link_cabi_realloc();
}
