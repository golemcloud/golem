use rustpython::{Interpreter, InterpreterBuilderExt, vm};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use vm::builtins::{PyBaseExceptionRef, PyBytesRef};
use vm::{AsObject, PyResult, Settings, VirtualMachine};

const HELP: &str = "usage: python [--cwd DIR] -- [-uBSEsPIO] [-c code | -m module | file | -] [arg ...]\n\
Execute Python using the embedded RustPython interpreter.\n\
No arguments or '-' reads a noninteractive script from stdin.\n\
Options: -u (unbuffered), -B (no bytecode), -S (no site), -E (ignore startup environment),\n\
         -s (no user site), -P (safe path), -I (isolated), -O/-OO (optimize), -h, -V.\n";

pub(crate) struct Stdio {
    input: Box<dyn Read>,
    output: Box<dyn Write>,
    error: Box<dyn Write>,
    failure: Option<String>,
}

impl Stdio {
    pub(crate) fn new(input: Box<dyn Read>, output: Box<dyn Write>, error: Box<dyn Write>) -> Self {
        Self {
            input,
            output,
            error,
            failure: None,
        }
    }

    fn write(&mut self, channel: u8, bytes: &[u8]) -> Result<(), String> {
        let writer = if channel == 1 {
            &mut self.output
        } else {
            &mut self.error
        };
        let result = writer.write_all(bytes).map_err(|e| e.to_string());
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
}

enum Mode {
    Command(String),
    Module(String),
    File(String),
    Stdin,
    Help,
    Version,
}

struct Options {
    mode: Mode,
    settings: Settings,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut settings = Settings::default();
    settings.install_signal_handlers = false;
    let mut index = 0;
    let mode = loop {
        let Some(arg) = args.get(index) else {
            settings.argv.push(String::new());
            break Mode::Stdin;
        };
        index += 1;
        match arg.as_str() {
            "-c" | "-m" => {
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{arg} requires an argument"))?
                    .clone();
                index += 1;
                settings
                    .argv
                    .push(if arg == "-c" { "-c" } else { "-m" }.into());
                break if arg == "-c" {
                    Mode::Command(value)
                } else {
                    Mode::Module(value)
                };
            }
            "-" => {
                settings.argv.push("-".into());
                break Mode::Stdin;
            }
            "--" => {
                let path = args.get(index).cloned();
                if let Some(path) = path {
                    index += 1;
                    settings.argv.push(path.clone());
                    break Mode::File(path);
                }
                settings.argv.push(String::new());
                break Mode::Stdin;
            }
            "-h" | "--help" => break Mode::Help,
            "-V" | "--version" => break Mode::Version,
            flag if flag.starts_with('-') => {
                for ch in flag[1..].chars() {
                    match ch {
                        'u' => settings.buffered_stdio = false,
                        'B' => settings.write_bytecode = false,
                        'S' => settings.import_site = false,
                        'E' => settings.ignore_environment = true,
                        's' => settings.user_site_directory = false,
                        'P' => settings.safe_path = true,
                        'I' => {
                            settings.isolated = true;
                            settings.ignore_environment = true;
                            settings.user_site_directory = false;
                            settings.safe_path = true;
                        }
                        'O' => settings.optimize = (settings.optimize + 1).min(2),
                        _ => return Err(format!("unsupported option: {flag}")),
                    }
                }
            }
            path => {
                settings.argv.push(path.into());
                break Mode::File(path.into());
            }
        }
    };
    settings.argv.extend_from_slice(&args[index..]);
    if !settings.ignore_environment {
        if std::env::var_os("PYTHONUNBUFFERED").is_some_and(|s| !s.is_empty()) {
            settings.buffered_stdio = false;
        }
        if std::env::var_os("PYTHONDONTWRITEBYTECODE").is_some_and(|s| !s.is_empty()) {
            settings.write_bytecode = false;
        }
        if let Some(paths) = std::env::var_os("PYTHONPATH") {
            settings
                .path_list
                .extend(std::env::split_paths(&paths).map(|p| p.to_string_lossy().into_owned()));
        }
    }
    Ok(Options { mode, settings })
}

struct WorkingDirectory(PathBuf);

impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

pub(crate) fn run(cwd: &str, args: &[String], streams: Rc<RefCell<Stdio>>) -> Result<i32, String> {
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            streams
                .borrow_mut()
                .write(2, format!("python: {message}\n{HELP}").as_bytes())?;
            return Ok(2);
        }
    };
    if matches!(options.mode, Mode::Help | Mode::Version) {
        let text = if matches!(options.mode, Mode::Help) {
            HELP.to_owned()
        } else {
            format!(
                "Python {}.{}.{} (RustPython)\n",
                vm::version::MAJOR,
                vm::version::MINOR,
                vm::version::MICRO
            )
        };
        streams.borrow_mut().write(1, text.as_bytes())?;
        return Ok(0);
    }
    if !Path::new(cwd).is_absolute() {
        streams
            .borrow_mut()
            .write(2, b"python: cwd must be an absolute path\n")?;
        return Ok(2);
    }
    let previous = std::env::current_dir().map_err(|e| format!("get working directory: {e}"))?;
    if let Err(error) = std::env::set_current_dir(cwd) {
        streams.borrow_mut().write(
            2,
            format!("python: cannot enter {cwd:?}: {error}\n").as_bytes(),
        )?;
        return Ok(2);
    }
    let _directory = WorkingDirectory(previous);
    let unbuffered = !options.settings.buffered_stdio;
    let import_site = options.settings.import_site;
    let safe_path = options.settings.safe_path;
    let interpreter = Interpreter::builder(options.settings).init_stdlib().build();
    let mut output_streams = Vec::new();
    let status = interpreter.enter(|vm| {
        let result = (|| -> PyResult<()> {
            install_stdio(vm, streams.clone(), unbuffered)?;
            output_streams = ["__stdout__", "__stderr__"]
                .into_iter()
                .map(|name| vm.sys_module.get_attr(name, vm))
                .collect::<PyResult<Vec<_>>>()?;
            if import_site {
                vm.import("site", 0)?;
            }
            let scope = vm.new_scope_with_main()?;
            if !safe_path {
                let path = match &options.mode {
                    Mode::File(path) => std::fs::canonicalize(path)
                        .unwrap_or_else(|_| PathBuf::from(path))
                        .parent()
                        .unwrap_or(Path::new(""))
                        .to_string_lossy()
                        .into_owned(),
                    Mode::Module(_) => cwd.to_owned(),
                    _ => String::new(),
                };
                vm.insert_sys_path(vm.new_pyobj(path))?;
            }
            match &options.mode {
                Mode::Command(code) => vm.run_string(scope, code, "<string>".into()).map(drop),
                Mode::Module(module) => vm.run_module(module),
                Mode::File(path) => {
                    if Path::new(path).is_dir() {
                        vm.insert_sys_path(vm.new_pyobj(path.clone()))?;
                        vm.import("runpy", 0)?
                            .get_attr("_run_module_as_main", vm)?
                            .call(("__main__", false), vm)?;
                        Ok(())
                    } else {
                        vm.run_any_file(scope, path)
                    }
                }
                Mode::Stdin => {
                    let text = vm
                        .sys_module
                        .get_attr("stdin", vm)?
                        .get_attr("read", vm)?
                        .call((), vm)?;
                    let text: vm::builtins::PyStrRef = text.try_into_value(vm)?;
                    vm.run_string(scope, text.as_str(), "<stdin>".into())
                        .map(drop)
                }
                Mode::Help | Mode::Version => unreachable!(),
            }
        })();
        match result {
            Ok(()) => 0,
            Err(error) => exit_status(vm, error),
        }
    });
    // Preserve RustPython's shutdown order, but flush attachments before module teardown.
    // finalize() clears sys before its last flush, which loses retained buffered output.
    interpreter.enter(|vm| {
        let flush = || {
            for name in ["stdout", "stderr"] {
                if let Ok(stream) = vm.sys_module.get_attr(name, vm) {
                    let _ = vm.call_method(&stream, "flush", ());
                }
            }
            for stream in &output_streams {
                let _ = vm.call_method(stream, "flush", ());
            }
        };
        flush();
        if let Ok(threading) = vm.import("threading", 0)
            && let Ok(shutdown) = threading.get_attr("_shutdown", vm)
            && let Err(error) = shutdown.call((), vm)
        {
            vm.run_unraisable(
                error,
                Some("Exception ignored in threading shutdown".into()),
                threading,
            );
        }
        vm.state
            .finalizing
            .store(true, std::sync::atomic::Ordering::Release);
        vm::stdlib::atexit::_run_exitfuncs(vm);
        flush();
        vm.finalize_modules();
    });
    drop(interpreter);
    let mut streams = streams.borrow_mut();
    if let Some(error) = streams.failure.take() {
        return Err(error);
    }
    Ok(status)
}

fn install_stdio(
    vm: &VirtualMachine,
    streams: Rc<RefCell<Stdio>>,
    unbuffered: bool,
) -> PyResult<()> {
    let scope = vm.new_scope_with_builtins();
    scope
        .globals
        .set_item("unbuffered", vm.new_pyobj(unbuffered), vm)?;
    let input = streams.clone();
    scope.globals.set_item(
        "read",
        vm.new_function(
            "read",
            move |size: usize, vm: &VirtualMachine| -> PyResult<PyBytesRef> {
                let mut bytes = vec![0; size.min(64 * 1024)];
                let mut input = input.borrow_mut();
                match input.input.read(&mut bytes) {
                    Ok(count) => {
                        bytes.truncate(count);
                        Ok(vm.ctx.new_bytes(bytes))
                    }
                    Err(error) => {
                        input.failure = Some(error.to_string());
                        Err(vm.new_os_error(error.to_string()))
                    }
                }
            },
        )
        .into(),
        vm,
    )?;
    scope.globals.set_item(
        "write",
        vm.new_function(
            "write",
            move |channel: u8, bytes: PyBytesRef, vm: &VirtualMachine| -> PyResult<()> {
                streams
                    .borrow_mut()
                    .write(channel, bytes.as_bytes())
                    .map_err(|e| vm.new_os_error(e))
            },
        )
        .into(),
        vm,
    )?;
    vm.run_string(scope, include_str!("stdio.py"), "<golem-stdio>".into())
        .map(drop)
}

fn exit_status(vm: &VirtualMachine, error: PyBaseExceptionRef) -> i32 {
    if error.fast_isinstance(vm.ctx.exceptions.system_exit) {
        if let Ok(code) = error.as_object().get_attr("code", vm) {
            if vm.is_none(&code) {
                return 0;
            }
            if let Some(integer) = code.downcast_ref::<vm::builtins::PyInt>() {
                // Python exit statuses are narrowed to the shell's unsigned byte.
                let value = integer.as_bigint() % 256i32;
                return value
                    .to_string()
                    .parse::<i32>()
                    .unwrap_or(0)
                    .rem_euclid(256);
            }
            if let Ok(message) = code.str(vm)
                && let Ok(stderr) = vm.sys_module.get_attr("stderr", vm)
            {
                let _ = stderr
                    .get_attr("write", vm)
                    .and_then(|f| f.call((format!("{}\n", message.as_str()),), vm));
            }
        }
        1
    } else if error.fast_isinstance(vm.ctx.exceptions.keyboard_interrupt) {
        vm.print_exception(error);
        130
    } else {
        vm.print_exception(error);
        1
    }
}
