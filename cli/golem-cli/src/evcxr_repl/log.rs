// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use rustyline::ExternalPrinter;
use std::fmt::Display;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

pub enum OutputMode {
    Stdout,
    Stderr,
    ExternalPrinter(Arc<Mutex<Box<dyn ExternalPrinter + Send>>>),
    None,
}

static OUTPUT_MODE: OnceLock<RwLock<OutputMode>> = OnceLock::new();

pub fn set_output(mode: OutputMode) {
    *output_mode().write().unwrap() = mode;
}

pub fn set_external_printer(printer: impl ExternalPrinter + Send + 'static) {
    set_output(OutputMode::ExternalPrinter(Arc::new(Mutex::new(Box::new(
        printer,
    )))));
}

pub fn log(message: impl Display) {
    write_message(message.to_string(), false);
}

pub fn logln(message: impl Display) {
    write_message(message.to_string(), true);
}

fn output_mode() -> &'static RwLock<OutputMode> {
    OUTPUT_MODE.get_or_init(|| RwLock::new(OutputMode::Stdout))
}

fn write_message(mut message: String, newline: bool) {
    if newline {
        message.push('\n');
    }

    let Ok(guard) = output_mode().read() else {
        return;
    };

    match &*guard {
        OutputMode::Stdout => {
            let mut out = io::stdout();
            let _ = out.write_all(message.as_bytes());
            let _ = out.flush();
        }
        OutputMode::Stderr => {
            let mut out = io::stderr();
            let _ = out.write_all(message.as_bytes());
            let _ = out.flush();
        }
        OutputMode::ExternalPrinter(printer) => {
            if let Ok(mut printer) = printer.lock() {
                let _ = printer.print(message);
            }
        }
        OutputMode::None => {}
    }
}
