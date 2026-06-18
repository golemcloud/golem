// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::durable_host::p3::{DurableP3, DurableP3View, wasi_cli_view};
use crate::workerctx::WorkerCtx;
use wasmtime::AsContextMut;
use wasmtime::component::{Access, FutureReader, Resource, StreamReader};
use wasmtime_wasi::cli::{WasiCli, WasiCliView};
use wasmtime_wasi::p3::bindings::cli::types::ErrorCode;
use wasmtime_wasi::p3::bindings::cli::{
    environment, exit, stderr, stdin, stdout, terminal_input, terminal_output, terminal_stderr,
    terminal_stdin, terminal_stdout,
};
use wasmtime_wasi::p3::cli::{TerminalInput, TerminalOutput};

impl<Ctx: WorkerCtx> environment::Host for DurableP3View<'_, Ctx> {
    fn get_environment(&mut self) -> wasmtime::Result<Vec<(String, String)>> {
        environment::Host::get_environment(&mut WasiCliView::cli(self.0))
    }

    fn get_arguments(&mut self) -> wasmtime::Result<Vec<String>> {
        environment::Host::get_arguments(&mut WasiCliView::cli(self.0))
    }

    fn get_initial_cwd(&mut self) -> wasmtime::Result<Option<String>> {
        environment::Host::get_initial_cwd(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> exit::Host for DurableP3View<'_, Ctx> {
    fn exit(&mut self, status: Result<(), ()>) -> wasmtime::Result<()> {
        exit::Host::exit(&mut WasiCliView::cli(self.0), status)
    }

    fn exit_with_code(&mut self, status_code: u8) -> wasmtime::Result<()> {
        exit::Host::exit_with_code(&mut WasiCliView::cli(self.0), status_code)
    }
}

impl<Ctx: WorkerCtx> terminal_input::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_input::HostTerminalInput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalInput>) -> wasmtime::Result<()> {
        terminal_input::HostTerminalInput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_output::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> terminal_output::HostTerminalOutput for DurableP3View<'_, Ctx> {
    fn drop(&mut self, rep: Resource<TerminalOutput>) -> wasmtime::Result<()> {
        terminal_output::HostTerminalOutput::drop(&mut WasiCliView::cli(self.0), rep)
    }
}

impl<Ctx: WorkerCtx> terminal_stdin::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdin(&mut self) -> wasmtime::Result<Option<Resource<TerminalInput>>> {
        terminal_stdin::Host::get_terminal_stdin(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stdout::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stdout(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        terminal_stdout::Host::get_terminal_stdout(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> terminal_stderr::Host for DurableP3View<'_, Ctx> {
    fn get_terminal_stderr(&mut self) -> wasmtime::Result<Option<Resource<TerminalOutput>>> {
        terminal_stderr::Host::get_terminal_stderr(&mut WasiCliView::cli(self.0))
    }
}

impl<Ctx: WorkerCtx> stdin::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> stdin::HostWithStore for DurableP3<Ctx> {
    fn read_via_stream<U>(
        mut store: Access<U, Self>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), ErrorCode>>)> {
        let store = Access::<U, WasiCli>::new(store.as_context_mut(), wasi_cli_view::<Ctx, U>);
        <WasiCli as stdin::HostWithStore>::read_via_stream(store)
    }
}

impl<Ctx: WorkerCtx> stdout::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> stdout::HostWithStore for DurableP3<Ctx> {
    fn write_via_stream<U>(
        mut store: Access<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        let store = Access::<U, WasiCli>::new(store.as_context_mut(), wasi_cli_view::<Ctx, U>);
        <WasiCli as stdout::HostWithStore>::write_via_stream(store, data)
    }
}

impl<Ctx: WorkerCtx> stderr::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> stderr::HostWithStore for DurableP3<Ctx> {
    fn write_via_stream<U>(
        mut store: Access<U, Self>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), ErrorCode>>> {
        let store = Access::<U, WasiCli>::new(store.as_context_mut(), wasi_cli_view::<Ctx, U>);
        <WasiCli as stderr::HostWithStore>::write_via_stream(store, data)
    }
}
