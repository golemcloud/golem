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

import { vi } from 'vitest';

// Global mocks which will be used within decorators,
// These host functionalities shouldn't run when decorators run.
// For example, getSelfMetadata is used in some decorators, however,
// it executes only when `initiate` is called.
// Also, these mocks are just place-holders. We can override the behavior
// per tests using functionalities overrides module
vi.mock('golem:api/host@1.1.7', () => ({
  getSelfMetadata: () => ({
    workerId: {
      componentId: { uuid: { highBits: 0n, lowBits: 0n } },
      workerName: 'change-this-by-overriding',
    },
    args: [],
    env: [],
    wasiConfigVars: [],
    status: 'running',
    componentVersion: 0n,
    retryCount: 0n,
  }),
}));

await import('./agentsInit');
