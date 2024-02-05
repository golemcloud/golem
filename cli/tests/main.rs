use crate::context::Context;
use libtest_mimic::{Arguments, Conclusion, Failed};
use std::sync::Arc;
use testcontainers::clients;

pub mod cli;
pub mod context;
mod template;
mod worker;

fn run(context: &Context<'_>) -> Conclusion {
    let args = Arguments::from_args();

    let context = Arc::new(context.info());

    let mut tests = Vec::new();

    tests.append(&mut template::all(context.clone()));
    tests.append(&mut worker::all(context.clone()));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();

    let docker = clients::Cli::default();
    let context = Context::start(&docker)?;

    let res = run(&context);

    drop(context);
    drop(docker);
    res.exit()
}
