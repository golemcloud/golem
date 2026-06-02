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

// TypeScript smoke agent for issue #3393.
//
// This file is intentionally a TypeScript SDK smoke: it proves the
// @readonly() decorator variants (default until-write, principal-aware
// via Principal parameter, ttl, and no-cache) compile, register, and are
// emitted into the component metadata, and that the resulting component
// builds via the QuickJS WASM injection / pre-initialization pipeline.
// Runtime cache semantics are covered by the Rust executor tests in
// golem-worker-executor/tests/readonly.rs and by the HTTP integration
// tests in integration-tests/tests/custom_api/readonly_http.rs.

import {
    BaseAgent,
    agent,
    endpoint,
    Principal,
    readonly,
} from '@golemcloud/golem-ts-sdk';

@agent({
    mount: '/ts-readonly-agents/{agentName}',
})
class TsReadonlyAgent extends BaseAgent {
    private count: number = 0;

    constructor(readonly agentName: string) {
        super();
    }

    // Non-read-only write, also exposed over HTTP so the TS agent could be
    // exercised end-to-end against the same fixtures as the Rust agent.
    @endpoint({ post: '/increment' })
    increment(): number {
        this.count += 1;
        return this.count;
    }

    // Default cache policy = 'until-write', principal-unaware.
    @readonly()
    @endpoint({ get: '/count' })
    getCount(): number {
        return this.count;
    }

    // Principal-aware: usesPrincipal is auto-derived from the Principal
    // parameter in the signature.
    @readonly()
    @endpoint({ get: '/count-for' })
    getCountFor(principal: Principal): number {
        return this.count;
    }

    // TTL cache policy.
    @readonly({ cache: { ttl: '2s' } })
    @endpoint({ get: '/ttl-count' })
    readOnlyWithTtl(): number {
        return this.count;
    }

    // No-cache: pure compute, no host calls, runs every invocation.
    @readonly({ cache: 'no-cache' })
    pureCompute(x: number, y: number): number {
        return Math.imul(x + y, 3);
    }
}
