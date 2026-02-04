// call_agent/mod.rs
// Call agent logic for routing requests
// Copyright 2024-2025 Golem Cloud

//! NOTE:
// Worker-related APIs were removed from `golem_service_base::api` as part
//! of a refactor. The call agent currently does not depend on worker APIs
//! and is kept minimal until new integrations are introduced.

pub struct CallAgent;

impl CallAgent {
    pub fn new() -> Self {
        Self
    }
}