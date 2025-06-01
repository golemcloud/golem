use rib::RibCompiler;
use crate::{RawRibScript, ReplPrinter};

// A projection of internal repl_state that could be useful
// for advanced customisation of REPL commands.
pub struct ReplContext {
    printer: Box<dyn ReplPrinter>,
    rib_script: RawRibScript,
    compiler: &'static RibCompiler,
}

impl ReplContext {
    pub(crate) fn new(printer: Box<dyn ReplPrinter>, compiler: &'static RibCompiler) -> Self {
        Self {
            printer,
            rib_script: RawRibScript::default(),
            compiler,
        }
    }

    pub fn get_printer(&self) -> &dyn ReplPrinter {
        self.printer.as_ref()
    }

    pub fn get_rib_script(&self) -> &RawRibScript {
        &self.rib_script
    }

    pub fn get_compiler(&self) -> &'static RibCompiler {
        self.compiler
    }
}

