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

use golem_rust::quota::QuotaToken;
use golem_rust::{agent_definition, agent_implementation};
use wstd::http::{Body, Client, Request};

#[agent_definition]
pub trait QuotaRpcReceiver {
    fn new(name: String) -> Self;

    async fn reserve_and_call_in_loop(&self, child_token: QuotaToken, host: String, port: u16, max_iterations: u64);
}

pub struct QuotaRpcReceiverImpl {
    _name: String
}

#[agent_implementation]
impl QuotaRpcReceiver for QuotaRpcReceiverImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name
        }
    }

    async fn reserve_and_call_in_loop(&self, token: QuotaToken, host: String, port: u16, max_iterations: u64) {
        for _ in 0..max_iterations {
            match token.reserve(1) {
                Err(_) => break,
                Ok(reservation) => {
                    let req = Request::get(format!("http://{host}:{port}/call"))
                        .body(Body::empty())
                        .unwrap();
                    let _ = Client::new().send(req).await.unwrap();
                    reservation.commit(1);
                }
            }
        }
    }
}

#[agent_definition]
pub trait QuotaRpcSender {
    fn new(name: String) -> Self;

    async fn split_and_loop(
        &self,
        resource_name: String,
        total_expected_use: u64,
        child_expected_use: u64,
        host: String,
        port: u16,
        max_iterations: u64,
    );
}

pub struct QuotaRpcSenderImpl {
    name: String,
}

#[agent_implementation]
impl QuotaRpcSender for QuotaRpcSenderImpl {
    fn new(name: String) -> Self {
        Self { name: name.clone() }
    }

    async fn split_and_loop(
        &self,
        resource_name: String,
        total_expected_use: u64,
        child_expected_use: u64,
        host: String,
        port: u16,
        max_iterations: u64,
    ) {
        let mut token = QuotaToken::new(&resource_name, total_expected_use);
        let child_token = token.split(child_expected_use);

        let receiver_name = format!("{}-receiver", self.name);
        let receiver = QuotaRpcReceiverClient::get(receiver_name);
        receiver.trigger_reserve_and_call_in_loop(child_token, host.clone(), port, max_iterations);

        for _ in 0..max_iterations {
            match token.reserve(1) {
                Err(_) => break,
                Ok(reservation) => {
                    let req = Request::get(format!("http://{host}:{port}/call"))
                        .body(Body::empty())
                        .unwrap();
                    Client::new().send(req).await.unwrap();
                    reservation.commit(1);
                }
            }
        }
    }
}
