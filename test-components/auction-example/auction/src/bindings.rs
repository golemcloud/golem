// Generated by `wit-bindgen` 0.25.0. DO NOT EDIT!
// Options used:
#[allow(dead_code)]
pub mod exports {
    #[allow(dead_code)]
    pub mod auction {
        #[allow(dead_code)]
        pub mod auction {
            #[allow(dead_code, clippy::all)]
            pub mod api {
                #[used]
                #[doc(hidden)]
                #[cfg(target_arch = "wasm32")]
                static __FORCE_SECTION_REF: fn() =
                    super::super::super::super::__link_custom_section_describing_imports;
                use super::super::super::super::_rt;
                #[derive(Clone)]
                pub struct BidderId {
                    pub bidder_id: _rt::String,
                }
                impl ::core::fmt::Debug for BidderId {
                    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        f.debug_struct("BidderId")
                            .field("bidder-id", &self.bidder_id)
                            .finish()
                    }
                }
                #[derive(Clone)]
                pub struct AuctionId {
                    pub auction_id: _rt::String,
                }
                impl ::core::fmt::Debug for AuctionId {
                    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        f.debug_struct("AuctionId")
                            .field("auction-id", &self.auction_id)
                            .finish()
                    }
                }
                pub type Deadline = u64;
                #[derive(Clone)]
                pub struct Auction {
                    pub auction_id: AuctionId,
                    pub name: _rt::String,
                    pub description: _rt::String,
                    pub limit_price: f32,
                    pub expiration: Deadline,
                }
                impl ::core::fmt::Debug for Auction {
                    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        f.debug_struct("Auction")
                            .field("auction-id", &self.auction_id)
                            .field("name", &self.name)
                            .field("description", &self.description)
                            .field("limit-price", &self.limit_price)
                            .field("expiration", &self.expiration)
                            .finish()
                    }
                }
                #[derive(Clone, Copy)]
                pub enum BidResult {
                    AuctionExpired,
                    PriceTooLow,
                    Success,
                }
                impl ::core::fmt::Debug for BidResult {
                    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                        match self {
                            BidResult::AuctionExpired => {
                                f.debug_tuple("BidResult::AuctionExpired").finish()
                            }
                            BidResult::PriceTooLow => {
                                f.debug_tuple("BidResult::PriceTooLow").finish()
                            }
                            BidResult::Success => f.debug_tuple("BidResult::Success").finish(),
                        }
                    }
                }
                /// an alternative interface for hosting multiple auctions in a single worker

                #[derive(Debug)]
                #[repr(transparent)]
                pub struct RunningAuction {
                    handle: _rt::Resource<RunningAuction>,
                }

                type _RunningAuctionRep<T> = Option<T>;

                impl RunningAuction {
                    /// Creates a new resource from the specified representation.
                    ///
                    /// This function will create a new resource handle by moving `val` onto
                    /// the heap and then passing that heap pointer to the component model to
                    /// create a handle. The owned handle is then returned as `RunningAuction`.
                    pub fn new<T: GuestRunningAuction>(val: T) -> Self {
                        Self::type_guard::<T>();
                        let val: _RunningAuctionRep<T> = Some(val);
                        let ptr: *mut _RunningAuctionRep<T> =
                            _rt::Box::into_raw(_rt::Box::new(val));
                        unsafe { Self::from_handle(T::_resource_new(ptr.cast())) }
                    }

                    /// Gets access to the underlying `T` which represents this resource.
                    pub fn get<T: GuestRunningAuction>(&self) -> &T {
                        let ptr = unsafe { &*self.as_ptr::<T>() };
                        ptr.as_ref().unwrap()
                    }

                    /// Gets mutable access to the underlying `T` which represents this
                    /// resource.
                    pub fn get_mut<T: GuestRunningAuction>(&mut self) -> &mut T {
                        let ptr = unsafe { &mut *self.as_ptr::<T>() };
                        ptr.as_mut().unwrap()
                    }

                    /// Consumes this resource and returns the underlying `T`.
                    pub fn into_inner<T: GuestRunningAuction>(self) -> T {
                        let ptr = unsafe { &mut *self.as_ptr::<T>() };
                        ptr.take().unwrap()
                    }

                    #[doc(hidden)]
                    pub unsafe fn from_handle(handle: u32) -> Self {
                        Self {
                            handle: _rt::Resource::from_handle(handle),
                        }
                    }

                    #[doc(hidden)]
                    pub fn take_handle(&self) -> u32 {
                        _rt::Resource::take_handle(&self.handle)
                    }

                    #[doc(hidden)]
                    pub fn handle(&self) -> u32 {
                        _rt::Resource::handle(&self.handle)
                    }

                    // It's theoretically possible to implement the `GuestRunningAuction` trait twice
                    // so guard against using it with two different types here.
                    #[doc(hidden)]
                    fn type_guard<T: 'static>() {
                        use core::any::TypeId;
                        static mut LAST_TYPE: Option<TypeId> = None;
                        unsafe {
                            assert!(!cfg!(target_feature = "threads"));
                            let id = TypeId::of::<T>();
                            match LAST_TYPE {
                                Some(ty) => assert!(
                                    ty == id,
                                    "cannot use two types with this resource type"
                                ),
                                None => LAST_TYPE = Some(id),
                            }
                        }
                    }

                    #[doc(hidden)]
                    pub unsafe fn dtor<T: 'static>(handle: *mut u8) {
                        Self::type_guard::<T>();
                        let _ = _rt::Box::from_raw(handle as *mut _RunningAuctionRep<T>);
                    }

                    fn as_ptr<T: GuestRunningAuction>(&self) -> *mut _RunningAuctionRep<T> {
                        RunningAuction::type_guard::<T>();
                        T::_resource_rep(self.handle()).cast()
                    }
                }

                /// A borrowed version of [`RunningAuction`] which represents a borrowed value
                /// with the lifetime `'a`.
                #[derive(Debug)]
                #[repr(transparent)]
                pub struct RunningAuctionBorrow<'a> {
                    rep: *mut u8,
                    _marker: core::marker::PhantomData<&'a RunningAuction>,
                }

                impl<'a> RunningAuctionBorrow<'a> {
                    #[doc(hidden)]
                    pub unsafe fn lift(rep: usize) -> Self {
                        Self {
                            rep: rep as *mut u8,
                            _marker: core::marker::PhantomData,
                        }
                    }

                    /// Gets access to the underlying `T` in this resource.
                    pub fn get<T: GuestRunningAuction>(&self) -> &T {
                        let ptr = unsafe { &mut *self.as_ptr::<T>() };
                        ptr.as_ref().unwrap()
                    }

                    // NB: mutable access is not allowed due to the component model allowing
                    // multiple borrows of the same resource.

                    fn as_ptr<T: 'static>(&self) -> *mut _RunningAuctionRep<T> {
                        RunningAuction::type_guard::<T>();
                        self.rep.cast()
                    }
                }

                unsafe impl _rt::WasmResource for RunningAuction {
                    #[inline]
                    unsafe fn drop(_handle: u32) {
                        #[cfg(not(target_arch = "wasm32"))]
                        unreachable!();

                        #[cfg(target_arch = "wasm32")]
                        {
                            #[link(wasm_import_module = "[export]auction:auction/api")]
                            extern "C" {
                                #[link_name = "[resource-drop]running-auction"]
                                fn drop(_: u32);
                            }

                            drop(_handle);
                        }
                    }
                }

                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_initialize_cabi<T: Guest>(
                    arg0: *mut u8,
                    arg1: usize,
                    arg2: *mut u8,
                    arg3: usize,
                    arg4: *mut u8,
                    arg5: usize,
                    arg6: f32,
                    arg7: i64,
                ) {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let len0 = arg1;
                    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
                    let len1 = arg3;
                    let bytes1 = _rt::Vec::from_raw_parts(arg2.cast(), len1, len1);
                    let len2 = arg5;
                    let bytes2 = _rt::Vec::from_raw_parts(arg4.cast(), len2, len2);
                    T::initialize(Auction {
                        auction_id: AuctionId {
                            auction_id: _rt::string_lift(bytes0),
                        },
                        name: _rt::string_lift(bytes1),
                        description: _rt::string_lift(bytes2),
                        limit_price: arg6,
                        expiration: arg7 as u64,
                    });
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_bid_cabi<T: Guest>(
                    arg0: *mut u8,
                    arg1: usize,
                    arg2: f32,
                ) -> i32 {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let len0 = arg1;
                    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
                    let result1 = T::bid(
                        BidderId {
                            bidder_id: _rt::string_lift(bytes0),
                        },
                        arg2,
                    );
                    let result2 = match result1 {
                        BidResult::AuctionExpired => 0i32,
                        BidResult::PriceTooLow => 1i32,
                        BidResult::Success => 2i32,
                    };
                    result2
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_close_auction_cabi<T: Guest>() -> *mut u8 {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let result0 = T::close_auction();
                    let ptr1 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
                    match result0 {
                        Some(e) => {
                            *ptr1.add(0).cast::<u8>() = (1i32) as u8;
                            let BidderId {
                                bidder_id: bidder_id2,
                            } = e;
                            let vec3 = (bidder_id2.into_bytes()).into_boxed_slice();
                            let ptr3 = vec3.as_ptr().cast::<u8>();
                            let len3 = vec3.len();
                            ::core::mem::forget(vec3);
                            *ptr1.add(8).cast::<usize>() = len3;
                            *ptr1.add(4).cast::<*mut u8>() = ptr3.cast_mut();
                        }
                        None => {
                            *ptr1.add(0).cast::<u8>() = (0i32) as u8;
                        }
                    };
                    ptr1
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn __post_return_close_auction<T: Guest>(arg0: *mut u8) {
                    let l0 = i32::from(*arg0.add(0).cast::<u8>());
                    match l0 {
                        0 => (),
                        _ => {
                            let l1 = *arg0.add(4).cast::<*mut u8>();
                            let l2 = *arg0.add(8).cast::<usize>();
                            _rt::cabi_dealloc(l1, l2, 1);
                        }
                    }
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_constructor_running_auction_cabi<T: GuestRunningAuction>(
                    arg0: *mut u8,
                    arg1: usize,
                    arg2: *mut u8,
                    arg3: usize,
                    arg4: *mut u8,
                    arg5: usize,
                    arg6: f32,
                    arg7: i64,
                ) -> i32 {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let len0 = arg1;
                    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
                    let len1 = arg3;
                    let bytes1 = _rt::Vec::from_raw_parts(arg2.cast(), len1, len1);
                    let len2 = arg5;
                    let bytes2 = _rt::Vec::from_raw_parts(arg4.cast(), len2, len2);
                    let result3 = RunningAuction::new(T::new(Auction {
                        auction_id: AuctionId {
                            auction_id: _rt::string_lift(bytes0),
                        },
                        name: _rt::string_lift(bytes1),
                        description: _rt::string_lift(bytes2),
                        limit_price: arg6,
                        expiration: arg7 as u64,
                    }));
                    (result3).take_handle() as i32
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_method_running_auction_bid_cabi<T: GuestRunningAuction>(
                    arg0: *mut u8,
                    arg1: *mut u8,
                    arg2: usize,
                    arg3: f32,
                ) -> i32 {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let len0 = arg2;
                    let bytes0 = _rt::Vec::from_raw_parts(arg1.cast(), len0, len0);
                    let result1 = T::bid(
                        RunningAuctionBorrow::lift(arg0 as u32 as usize).get(),
                        BidderId {
                            bidder_id: _rt::string_lift(bytes0),
                        },
                        arg3,
                    );
                    let result2 = match result1 {
                        BidResult::AuctionExpired => 0i32,
                        BidResult::PriceTooLow => 1i32,
                        BidResult::Success => 2i32,
                    };
                    result2
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_method_running_auction_close_cabi<T: GuestRunningAuction>(
                    arg0: *mut u8,
                ) -> *mut u8 {
                    #[cfg(target_arch = "wasm32")]
                    _rt::run_ctors_once();
                    let result0 = T::close(RunningAuctionBorrow::lift(arg0 as u32 as usize).get());
                    let ptr1 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
                    match result0 {
                        Some(e) => {
                            *ptr1.add(0).cast::<u8>() = (1i32) as u8;
                            let BidderId {
                                bidder_id: bidder_id2,
                            } = e;
                            let vec3 = (bidder_id2.into_bytes()).into_boxed_slice();
                            let ptr3 = vec3.as_ptr().cast::<u8>();
                            let len3 = vec3.len();
                            ::core::mem::forget(vec3);
                            *ptr1.add(8).cast::<usize>() = len3;
                            *ptr1.add(4).cast::<*mut u8>() = ptr3.cast_mut();
                        }
                        None => {
                            *ptr1.add(0).cast::<u8>() = (0i32) as u8;
                        }
                    };
                    ptr1
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn __post_return_method_running_auction_close<T: GuestRunningAuction>(
                    arg0: *mut u8,
                ) {
                    let l0 = i32::from(*arg0.add(0).cast::<u8>());
                    match l0 {
                        0 => (),
                        _ => {
                            let l1 = *arg0.add(4).cast::<*mut u8>();
                            let l2 = *arg0.add(8).cast::<usize>();
                            _rt::cabi_dealloc(l1, l2, 1);
                        }
                    }
                }
                pub trait Guest {
                    type RunningAuction: GuestRunningAuction;
                    fn initialize(auction: Auction);
                    fn bid(bidder_id: BidderId, price: f32) -> BidResult;
                    fn close_auction() -> Option<BidderId>;
                }
                pub trait GuestRunningAuction: 'static {
                    #[doc(hidden)]
                    unsafe fn _resource_new(val: *mut u8) -> u32
                    where
                        Self: Sized,
                    {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = val;
                            unreachable!();
                        }

                        #[cfg(target_arch = "wasm32")]
                        {
                            #[link(wasm_import_module = "[export]auction:auction/api")]
                            extern "C" {
                                #[link_name = "[resource-new]running-auction"]
                                fn new(_: *mut u8) -> u32;
                            }
                            new(val)
                        }
                    }

                    #[doc(hidden)]
                    fn _resource_rep(handle: u32) -> *mut u8
                    where
                        Self: Sized,
                    {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let _ = handle;
                            unreachable!();
                        }

                        #[cfg(target_arch = "wasm32")]
                        {
                            #[link(wasm_import_module = "[export]auction:auction/api")]
                            extern "C" {
                                #[link_name = "[resource-rep]running-auction"]
                                fn rep(_: u32) -> *mut u8;
                            }
                            unsafe { rep(handle) }
                        }
                    }

                    fn new(auction: Auction) -> Self;
                    fn bid(&self, bidder_id: BidderId, price: f32) -> BidResult;
                    fn close(&self) -> Option<BidderId>;
                }
                #[doc(hidden)]

                macro_rules! __export_auction_auction_api_cabi{
  ($ty:ident with_types_in $($path_to_types:tt)*) => (const _: () = {

    #[export_name = "auction:auction/api#initialize"]
    unsafe extern "C" fn export_initialize(arg0: *mut u8,arg1: usize,arg2: *mut u8,arg3: usize,arg4: *mut u8,arg5: usize,arg6: f32,arg7: i64,) {
      $($path_to_types)*::_export_initialize_cabi::<$ty>(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
    }
    #[export_name = "auction:auction/api#bid"]
    unsafe extern "C" fn export_bid(arg0: *mut u8,arg1: usize,arg2: f32,) -> i32 {
      $($path_to_types)*::_export_bid_cabi::<$ty>(arg0, arg1, arg2)
    }
    #[export_name = "auction:auction/api#close-auction"]
    unsafe extern "C" fn export_close_auction() -> *mut u8 {
      $($path_to_types)*::_export_close_auction_cabi::<$ty>()
    }
    #[export_name = "cabi_post_auction:auction/api#close-auction"]
    unsafe extern "C" fn _post_return_close_auction(arg0: *mut u8,) {
      $($path_to_types)*::__post_return_close_auction::<$ty>(arg0)
    }
    #[export_name = "auction:auction/api#[constructor]running-auction"]
    unsafe extern "C" fn export_constructor_running_auction(arg0: *mut u8,arg1: usize,arg2: *mut u8,arg3: usize,arg4: *mut u8,arg5: usize,arg6: f32,arg7: i64,) -> i32 {
      $($path_to_types)*::_export_constructor_running_auction_cabi::<<$ty as $($path_to_types)*::Guest>::RunningAuction>(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
    }
    #[export_name = "auction:auction/api#[method]running-auction.bid"]
    unsafe extern "C" fn export_method_running_auction_bid(arg0: *mut u8,arg1: *mut u8,arg2: usize,arg3: f32,) -> i32 {
      $($path_to_types)*::_export_method_running_auction_bid_cabi::<<$ty as $($path_to_types)*::Guest>::RunningAuction>(arg0, arg1, arg2, arg3)
    }
    #[export_name = "auction:auction/api#[method]running-auction.close"]
    unsafe extern "C" fn export_method_running_auction_close(arg0: *mut u8,) -> *mut u8 {
      $($path_to_types)*::_export_method_running_auction_close_cabi::<<$ty as $($path_to_types)*::Guest>::RunningAuction>(arg0)
    }
    #[export_name = "cabi_post_auction:auction/api#[method]running-auction.close"]
    unsafe extern "C" fn _post_return_method_running_auction_close(arg0: *mut u8,) {
      $($path_to_types)*::__post_return_method_running_auction_close::<<$ty as $($path_to_types)*::Guest>::RunningAuction>(arg0)
    }

    const _: () = {
      #[doc(hidden)]
      #[export_name = "auction:auction/api#[dtor]running-auction"]
      #[allow(non_snake_case)]
      unsafe extern "C" fn dtor(rep: *mut u8) {
        $($path_to_types)*::RunningAuction::dtor::<
        <$ty as $($path_to_types)*::Guest>::RunningAuction
        >(rep)
      }
    };

  };);
}
                #[doc(hidden)]
                pub(crate) use __export_auction_auction_api_cabi;
                #[repr(align(4))]
                struct _RetArea([::core::mem::MaybeUninit<u8>; 12]);
                static mut _RET_AREA: _RetArea = _RetArea([::core::mem::MaybeUninit::uninit(); 12]);
            }
        }
    }
}
mod _rt {
    pub use alloc_crate::string::String;

    use core::fmt;
    use core::marker;
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

    /// A type which represents a component model resource, either imported or
    /// exported into this component.
    ///
    /// This is a low-level wrapper which handles the lifetime of the resource
    /// (namely this has a destructor). The `T` provided defines the component model
    /// intrinsics that this wrapper uses.
    ///
    /// One of the chief purposes of this type is to provide `Deref` implementations
    /// to access the underlying data when it is owned.
    ///
    /// This type is primarily used in generated code for exported and imported
    /// resources.
    #[repr(transparent)]
    pub struct Resource<T: WasmResource> {
        // NB: This would ideally be `u32` but it is not. The fact that this has
        // interior mutability is not exposed in the API of this type except for the
        // `take_handle` method which is supposed to in theory be private.
        //
        // This represents, almost all the time, a valid handle value. When it's
        // invalid it's stored as `u32::MAX`.
        handle: AtomicU32,
        _marker: marker::PhantomData<T>,
    }

    /// A trait which all wasm resources implement, namely providing the ability to
    /// drop a resource.
    ///
    /// This generally is implemented by generated code, not user-facing code.
    #[allow(clippy::missing_safety_doc)]
    pub unsafe trait WasmResource {
        /// Invokes the `[resource-drop]...` intrinsic.
        unsafe fn drop(handle: u32);
    }

    impl<T: WasmResource> Resource<T> {
        #[doc(hidden)]
        pub unsafe fn from_handle(handle: u32) -> Self {
            debug_assert!(handle != u32::MAX);
            Self {
                handle: AtomicU32::new(handle),
                _marker: marker::PhantomData,
            }
        }

        /// Takes ownership of the handle owned by `resource`.
        ///
        /// Note that this ideally would be `into_handle` taking `Resource<T>` by
        /// ownership. The code generator does not enable that in all situations,
        /// unfortunately, so this is provided instead.
        ///
        /// Also note that `take_handle` is in theory only ever called on values
        /// owned by a generated function. For example a generated function might
        /// take `Resource<T>` as an argument but then call `take_handle` on a
        /// reference to that argument. In that sense the dynamic nature of
        /// `take_handle` should only be exposed internally to generated code, not
        /// to user code.
        #[doc(hidden)]
        pub fn take_handle(resource: &Resource<T>) -> u32 {
            resource.handle.swap(u32::MAX, Relaxed)
        }

        #[doc(hidden)]
        pub fn handle(resource: &Resource<T>) -> u32 {
            resource.handle.load(Relaxed)
        }
    }

    impl<T: WasmResource> fmt::Debug for Resource<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Resource")
                .field("handle", &self.handle)
                .finish()
        }
    }

    impl<T: WasmResource> Drop for Resource<T> {
        fn drop(&mut self) {
            unsafe {
                match self.handle.load(Relaxed) {
                    // If this handle was "taken" then don't do anything in the
                    // destructor.
                    u32::MAX => {}

                    // ... but otherwise do actually destroy it with the imported
                    // component model intrinsic as defined through `T`.
                    other => T::drop(other),
                }
            }
        }
    }
    pub use alloc_crate::boxed::Box;

    #[cfg(target_arch = "wasm32")]
    pub fn run_ctors_once() {
        wit_bindgen_rt::run_ctors_once();
    }
    pub use alloc_crate::vec::Vec;
    pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
        if cfg!(debug_assertions) {
            String::from_utf8(bytes).unwrap()
        } else {
            String::from_utf8_unchecked(bytes)
        }
    }
    pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
        if size == 0 {
            return;
        }
        let layout = alloc::Layout::from_size_align_unchecked(size, align);
        alloc::dealloc(ptr as *mut u8, layout);
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

macro_rules! __export_auction_impl {
  ($ty:ident) => (self::export!($ty with_types_in self););
  ($ty:ident with_types_in $($path_to_types_root:tt)*) => (
  $($path_to_types_root)*::exports::auction::auction::api::__export_auction_auction_api_cabi!($ty with_types_in $($path_to_types_root)*::exports::auction::auction::api);
  )
}
#[doc(inline)]
pub(crate) use __export_auction_impl as export;

#[cfg(target_arch = "wasm32")]
#[link_section = "component-type:wit-bindgen:0.25.0:auction:encoded world"]
#[doc(hidden)]
pub static __WIT_BINDGEN_COMPONENT_TYPE: [u8; 663] = *b"\
\0asm\x0d\0\x01\0\0\x19\x16wit-component-encoding\x04\0\x07\x99\x04\x01A\x02\x01\
A\x02\x01B\x1a\x01r\x01\x09bidder-ids\x04\0\x09bidder-id\x03\0\0\x01r\x01\x0aauc\
tion-ids\x04\0\x0aauction-id\x03\0\x02\x01w\x04\0\x08deadline\x03\0\x04\x01r\x05\
\x0aauction-id\x03\x04names\x0bdescriptions\x0blimit-pricev\x0aexpiration\x05\x04\
\0\x07auction\x03\0\x06\x01q\x03\x0fauction-expired\0\0\x0dprice-too-low\0\0\x07\
success\0\0\x04\0\x0abid-result\x03\0\x08\x04\0\x0frunning-auction\x03\x01\x01i\x0a\
\x01@\x01\x07auction\x07\0\x0b\x04\0\x1c[constructor]running-auction\x01\x0c\x01\
h\x0a\x01@\x03\x04self\x0d\x09bidder-id\x01\x05pricev\0\x09\x04\0\x1b[method]run\
ning-auction.bid\x01\x0e\x01k\x01\x01@\x01\x04self\x0d\0\x0f\x04\0\x1d[method]ru\
nning-auction.close\x01\x10\x01@\x01\x07auction\x07\x01\0\x04\0\x0ainitialize\x01\
\x11\x01@\x02\x09bidder-id\x01\x05pricev\0\x09\x04\0\x03bid\x01\x12\x01@\0\0\x0f\
\x04\0\x0dclose-auction\x01\x13\x04\x01\x13auction:auction/api\x05\0\x04\x01\x17\
auction:auction/auction\x04\0\x0b\x0d\x01\0\x07auction\x03\0\0\0G\x09producers\x01\
\x0cprocessed-by\x02\x0dwit-component\x070.208.1\x10wit-bindgen-rust\x060.25.0";

#[inline(never)]
#[doc(hidden)]
#[cfg(target_arch = "wasm32")]
pub fn __link_custom_section_describing_imports() {
    wit_bindgen_rt::maybe_link_cabi_realloc();
}