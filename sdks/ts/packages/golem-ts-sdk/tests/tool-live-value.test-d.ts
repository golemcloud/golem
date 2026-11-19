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

// A Principal, a Caller and a TokenProvenance are live values: a tool refuses them in an option,
// a positional, a tail, a result and an error payload, as a method does.
import { z } from 'zod/v4';
import {
  toolDefinition,
  type Caller,
  type Principal,
  type TokenProvenance,
} from '../dist/index.mjs';

declare const liveCaller: z.ZodType<Caller>;
declare const livePrincipal: z.ZodType<Principal>;
declare const liveToken: z.ZodType<TokenProvenance>;

// @ts-expect-error a Caller cannot be a tool option
toolDefinition('option-tool').body((b) => b.option('who', liveCaller).returns(z.void()));

// @ts-expect-error a TokenProvenance cannot be a tool positional
toolDefinition('positional-tool').body((b) => b.positional('token', liveToken).returns(z.void()));

// @ts-expect-error a Principal cannot be a tool tail
toolDefinition('tail-tool').body((b) => b.tail('who', livePrincipal).returns(z.void()));

// @ts-expect-error a Principal cannot be a tool result
toolDefinition('result-tool').body((b) => b.returns(livePrincipal));

// @ts-expect-error a Principal cannot be nested in a tool result
toolDefinition('nested-tool').body((b) => b.returns(z.object({ who: livePrincipal })));

toolDefinition('error-tool').body((b) =>
  // @ts-expect-error a Principal cannot be a tool error payload
  b.error('bad', { payload: livePrincipal }).returns(z.void()),
);

// Plain data is accepted.
toolDefinition('ok-tool').body((b) =>
  b
    .option('name', z.string())
    .positional('count', z.number())
    .returns(z.object({ ok: z.boolean() })),
);
