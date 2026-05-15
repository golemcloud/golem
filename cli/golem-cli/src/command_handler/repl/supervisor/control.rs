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

use super::SupervisorEvent;
use anyhow::Context;
use serde_derive::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc::{self, Sender};
use std::thread;

#[derive(Debug)]
pub struct ControlServer {
    listener: TcpListener,
    addr: SocketAddr,
    token: String,
}

impl ControlServer {
    pub fn start(token: String) -> anyhow::Result<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("Failed to bind REPL control server")?;
        let addr = listener.local_addr()?;
        Ok(Self {
            listener,
            addr,
            token,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn spawn_request_reader(self, event_tx: Sender<SupervisorEvent>) {
        thread::spawn(move || {
            loop {
                let Ok((stream, _)) = self.listener.accept() else {
                    return;
                };
                let Ok(mut writer) = stream.try_clone() else {
                    return;
                };
                let reader = BufReader::new(stream);

                for line in reader.lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let request = match serde_json::from_str::<ControlRequest>(&line) {
                        Ok(request) => request,
                        Err(err) => {
                            let _ = write_response(
                                &mut writer,
                                &ControlResponse::error(None, format!("invalid request: {err}")),
                            );
                            continue;
                        }
                    };

                    if request.token != self.token {
                        let _ = write_response(
                            &mut writer,
                            &ControlResponse::error(Some(request.id), "invalid control token"),
                        );
                        continue;
                    }

                    match request.kind.as_str() {
                        "runCli" => {
                            let (response_tx, response_rx) = mpsc::channel();
                            if event_tx
                                .send(SupervisorEvent::RunCli(RunCliSupervisorRequest {
                                    args: request.args,
                                    response: response_tx,
                                }))
                                .is_err()
                            {
                                return;
                            }
                            let response = match response_rx.recv() {
                                Ok(response) => ControlResponse::cli_result(request.id, response),
                                Err(err) => ControlResponse::error(
                                    Some(request.id),
                                    format!("failed to receive CLI result: {err}"),
                                ),
                            };
                            let _ = write_response(&mut writer, &response);
                        }
                        _ => {
                            let _ = write_response(
                                &mut writer,
                                &ControlResponse::error(
                                    Some(request.id),
                                    format!("unknown request type: {}", request.kind),
                                ),
                            );
                        }
                    }
                }
            }
        });
    }
}

pub struct RunCliSupervisorRequest {
    pub args: Vec<String>,
    pub response: Sender<RunCliResponse>,
}

pub struct RunCliResponse {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlRequest {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    token: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlResponse {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    ok: bool,
    code: Option<i32>,
    stdout: Option<String>,
    stderr: Option<String>,
    error: Option<String>,
}

impl ControlResponse {
    fn cli_result(id: String, result: RunCliResponse) -> Self {
        Self {
            kind: "cliResult".to_string(),
            id: Some(id),
            ok: result.ok,
            code: result.code,
            stdout: result.stdout,
            stderr: result.stderr,
            error: None,
        }
    }

    fn error(id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            kind: "error".to_string(),
            id,
            ok: false,
            code: None,
            stdout: None,
            stderr: None,
            error: Some(message.into()),
        }
    }
}

fn write_response(writer: &mut impl Write, response: &ControlResponse) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
