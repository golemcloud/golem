#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::*;
// Import for using common lib (also see Cargo.toml for adding the dependency):
// use common_lib::example_common_function;
use std::{cell::RefCell, time::Duration};
use wasi::clocks::monotonic_clock::subscribe_duration;
use wasi_async_runtime::{block_on, Reactor};

/// This is one of any number of data types that our application
/// uses. Golem will take care to persist all application state,
/// whether that state is local to a function being executed or
/// global across the entire program.
///
/// The `reactor` is set on each exported async function boundary, so it does not
/// need to be propagated everywhere a pollable needs to be awaited..
#[derive(Default)]
pub struct State {
    pub inner: RefCell<InnerState>,
}

#[derive(Default)]
pub struct InnerState {
    reactor: Option<Reactor>,
    total: u64,
}

/// This holds the state of our application.
static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
pub fn get_state() -> &'static State {
    unsafe {
        if STATE.is_none() {
            STATE = Some(State::default());
        }
        STATE.as_ref().unwrap()
    }
}

struct Component;

impl Guest for Component {
    /// Updates the component's state by adding the given value to the total.
    fn add(value: u64) {
        block_on(|reactor| async move {
            get_state().inner.borrow_mut().reactor = Some(reactor);
            async_add(value).await
        })
    }

    /// Returns the current total.
    fn get() -> u64 {
        block_on(|reactor| async move {
            get_state().inner.borrow_mut().reactor = Some(reactor);
            async_get().await
        })
    }
}

async fn async_add(value: u64) {
    let mut state = get_state().inner.borrow_mut();
    state.total += value;
}

async fn async_get() -> u64 {
    let state = get_state().inner.borrow_mut();

    // Call code from shared lib
    // println!("{}", example_common_function());

    state.total
}

/// An async sleep function
pub async fn sleep(duration: Duration) {
    let reactor = get_state().inner.borrow().reactor.clone().unwrap();

    let pollable = subscribe_duration(duration.as_nanos() as u64);
    reactor.wait_for(pollable).await;
}

bindings::export!(Component with_types_in bindings);
