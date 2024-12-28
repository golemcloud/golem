use std::default::Default;
use std::ops::{Deref, DerefMut};

#[derive(Debug)]
pub struct Dirty<T>(pub T);

impl<T> Reset for Dirty<T> {
    fn reset(&mut self) {
        // Do nothing!
    }
}

unsafe impl<T: Send> Send for Dirty<T> {}
unsafe impl<T: Sync> Sync for Dirty<T> {}

impl<T> Deref for Dirty<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Dirty<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

/// Resetting an object reverts that object back to a default state.
pub trait Reset {
    fn reset(&mut self);
}

// For most of the stdlib collections, this will "clear" the collection
// without deallocating.
impl<T: Default + Clone> Reset for T {
    fn reset(&mut self) {
        self.clone_from(&Default::default());
    }
}
