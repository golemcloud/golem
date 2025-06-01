use crate::{RawRibScript, ReplPrinter};
use rib::RibCompiler;

// A projection of internal repl_state that could be useful
// for advanced customisation of REPL commands.
pub struct ReplContext<'a> {
    printer: &'a dyn ReplPrinter,
    rib_script: &'a RawRibScript,
    compiler: &'a RibCompiler,
}

impl<'a> ReplContext<'a> {
    pub(crate) fn new(
        printer: &'a dyn ReplPrinter,
        rib_script: &'a RawRibScript,
        compiler: &'a RibCompiler,
    ) -> Self {
        Self {
            printer,
            rib_script,
            compiler,
        }
    }

    pub fn get_printer(&self) -> &dyn ReplPrinter {
        self.printer
    }

    pub fn get_rib_script(&self) -> &RawRibScript {
        &self.rib_script
    }

    pub fn get_rib_compiler(&self) -> &RibCompiler {
        &self.compiler
    }
}
