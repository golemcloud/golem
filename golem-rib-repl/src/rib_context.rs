use crate::repl_state::ReplState;
use crate::rib_edit::RibEdit;
use crate::{RawRibScript, ReplPrinter};
use rib::RibCompiler;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use std::sync::RwLockReadGuard;

// A projection of internal repl_state that could be useful
// for advanced customisation of REPL commands.
pub struct ReplContext<'a> {
    printer: &'a dyn ReplPrinter,
    repl_state: &'a ReplState,
    editor: &'a mut Editor<RibEdit, DefaultHistory>,
}

impl<'a> ReplContext<'a> {
    pub(crate) fn new(
        printer: &'a dyn ReplPrinter,
        repl_state: &'a ReplState,
        editor: &'a mut Editor<RibEdit, DefaultHistory>,
    ) -> Self {
        Self {
            printer,
            repl_state,
            editor,
        }
    }

    pub fn get_printer(&self) -> &dyn ReplPrinter {
        self.printer
    }

    pub fn clear_state(&self) {
        self.repl_state.clear()
    }

    pub fn clear_history(&mut self) {
        self.editor.clear_history().unwrap();
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
