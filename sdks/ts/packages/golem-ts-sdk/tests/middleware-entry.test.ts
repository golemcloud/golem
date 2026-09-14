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

import * as middleware from '@golemcloud/golem-ts-sdk/middleware';
import { describe, expect, it } from 'vitest';

describe('middleware package entry', () => {
  it('exports host-neutral authoring without the ambient tool client', () => {
    expect(middleware.toolDefinition).toBeTypeOf('function');
    expect(middleware.universalToolMiddleware).toBeTypeOf('function');
    expect(middleware.ToolInvokeError).toBeTypeOf('function');
    expect(middleware).not.toHaveProperty('client');
    expect(middleware).not.toHaveProperty('ToolCallError');
  });
});
