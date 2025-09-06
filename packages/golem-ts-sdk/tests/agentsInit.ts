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

const SampleAgentModuleName = './sampleAgents';

import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { Metadata } from '../.metadata/generated-types';
import { TypescriptTypeRegistry } from '../src';

// This setup is ran before every test suite (vitest worker)
// and represents the entry point of any code-first user code
TypescriptTypeRegistry.register(Metadata);

await import(SampleAgentModuleName);

console.log(
  `✅ Test-setup: Loaded type-script types in ${SampleAgentModuleName} and the following agents are registered: ${getAgentClassNamesInMetadata()}`,
);

function getAgentClassNamesInMetadata() {
  return Array.from(TypeMetadata.getAll())
    .map((entry) => entry[0])
    .join(', ');
}
