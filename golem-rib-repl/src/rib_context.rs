use std::sync::RwLockReadGuard;
use crate::{RawRibScript, ReplPrinter};
use rib::RibCompiler;
use crate::repl_state::ReplState;

// A projection of internal repl_state that could be useful
// for advanced customisation of REPL commands.
pub struct ReplContext<'a> {
    printer: &'a dyn ReplPrinter,
    repl_state: &'a ReplState,
}

impl<'a> ReplContext<'a> {
    pub(crate) fn new(
        printer: &'a dyn ReplPrinter,
        repl_state: &'a ReplState,
    ) -> Self {
        Self {
            printer,
            repl_state,
        }
    }

    pub fn get_printer(&self) -> &dyn ReplPrinter {
        self.printer
    }

    pub fn clear(&self) {
        self.repl_state.clear()
    }

    pub fn get_new_rib_script(&self, rib: &str) -> RawRibScript {
        let rib_script = self.repl_state.rib_script();
        let result = &*rib_script;
        let mut result = result.clone();
        result.push(rib);
        result
    }

    pub fn get_rib_compiler(&self) -> RwLockReadGuard<RibCompiler> {
        self.repl_state.rib_compiler()
    }

}
