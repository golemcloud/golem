// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::InputStream;
use crate::agentic::agent_impl::Component;
use crate::agentic::tool_registry::{get_all_tools, get_tool_by_name, get_tool_invoker_by_name};
use crate::golem_agentic::exports::golem::tool::guest::{
    Guest, InvocationResult, Tool, ToolError, TypedSchemaValue,
};
use crate::golem_agentic::golem::agent::common::Principal;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct OutputStreamState {
    chunks: VecDeque<Vec<u8>>,
    producer_closed: bool,
    reader_closed: bool,
    waker: Option<Waker>,
}

/// Writable stdout passed to tool implementations.
///
/// Writes are buffered until the tool invocation returns its readable P3
/// stream to the caller. Buffer acceptance does not guarantee delivery if the
/// reader closes before the forwarder writes the buffered bytes.
pub struct OutputStream {
    state: Rc<RefCell<OutputStreamState>>,
}

impl OutputStream {
    /// Buffers a chunk unless downstream closure has already been observed.
    ///
    /// Returns the full chunk when closure was previously observed. A chunk
    /// accepted before the forwarder observes closure can still be discarded.
    pub fn write(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        if bytes.is_empty() {
            return Vec::new();
        }

        let waker = {
            let mut state = self.state.borrow_mut();
            if state.reader_closed {
                return bytes;
            }
            state.chunks.push_back(bytes);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Vec::new()
    }

    /// Buffers all bytes and returns them if closure was previously observed.
    pub async fn write_all(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        self.write(bytes)
    }

    /// Buffers one byte and returns it if closure was previously observed.
    pub async fn write_one(&mut self, byte: u8) -> Option<u8> {
        self.write(vec![byte]).pop()
    }
}

impl Drop for OutputStream {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.state.borrow_mut();
            state.producer_closed = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

struct NextOutputChunk {
    state: Rc<RefCell<OutputStreamState>>,
}

impl Future for NextOutputChunk {
    type Output = Option<Vec<u8>>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if let Some(chunk) = state.chunks.pop_front() {
            Poll::Ready(Some(chunk))
        } else if state.producer_closed || state.reader_closed {
            Poll::Ready(None)
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

#[doc(hidden)]
pub fn new_tool_stdout() -> (OutputStream, InputStream) {
    let state = Rc::new(RefCell::new(OutputStreamState::default()));
    let (mut writer, reader) = crate::wit_stream::new::<u8>();
    let forward_state = Rc::clone(&state);
    wit_bindgen::spawn_local(async move {
        while let Some(chunk) = (NextOutputChunk {
            state: Rc::clone(&forward_state),
        })
        .await
        {
            if !writer.write_all(chunk).await.is_empty() {
                let mut state = forward_state.borrow_mut();
                state.reader_closed = true;
                state.chunks.clear();
                return;
            }
        }
    });
    (OutputStream { state }, reader)
}

impl Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        Ok(get_all_tools())
    }

    fn get_tool(name: String) -> Result<Tool, ToolError> {
        get_tool_by_name(&name).ok_or(ToolError::InvalidToolName(name))
    }

    async fn invoke(
        tool_name: String,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        principal: Principal,
    ) -> Result<InvocationResult, ToolError> {
        let invoker = get_tool_invoker_by_name(&tool_name)
            .ok_or_else(|| ToolError::InvalidToolName(tool_name.clone()))?;
        invoker(command_path, input, stdin, principal).await
    }
}
