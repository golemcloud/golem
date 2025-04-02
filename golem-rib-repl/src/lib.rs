mod compiler;
pub mod dependency_manager;
#[cfg(feature = "embedded")]
pub mod embedded;
pub mod invoke;
pub mod repl_printer;
mod repl_state;
mod rib_edit;
pub mod rib_repl;
mod value_generator;

#[cfg(test)]
test_r::enable!();
