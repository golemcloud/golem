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
import { z } from 'zod';
import {
  Caller,
  defineAgent,
  method,
  PrincipalRef,
  s,
  type InvocationContext,
  type Principal,
  type TokenProvenance,
} from '../dist/index.mjs';

// A PrincipalRef is plain data in every position.
method({ input: { who: s.principalRef() }, returns: s.principalRef() });
method({
  input: { data: z.object({ who: s.principalRef() }) },
  returns: z.object({ who: s.principalRef() }),
});
defineAgent({ name: 'Ok', id: { owner: s.principalRef() }, methods: {} });

// A handler receives the decoded input and the invocation context.
defineAgent({
  name: 'Handler',
  id: {},
  methods: { run: method({ input: { n: z.number() }, returns: z.string() }) },
}).implement({
  init: (ctx) => {
    const caller: Caller = ctx.caller;
    void caller;
    return {};
  },
  methods: {
    run({ n }, ctx: InvocationContext) {
      const principal: Principal = ctx.caller.original;
      void principal;
      return `${n}`;
    },
  },
});

declare const live: z.ZodType<Principal>;
declare const liveCaller: z.ZodType<Caller>;
declare const liveToken: z.ZodType<TokenProvenance>;

// @ts-expect-error a Principal cannot be a method input
method({ input: { who: live }, returns: z.void() });
// @ts-expect-error a Caller cannot be a method input
method({ input: { who: liveCaller }, returns: z.void() });
// @ts-expect-error a TokenProvenance cannot be a method input
method({ input: { token: liveToken }, returns: z.void() });
// @ts-expect-error a Principal cannot be inside an input object
method({ input: { data: z.object({ who: live }) }, returns: z.void() });
// @ts-expect-error a Principal cannot be inside an input array
method({ input: { data: z.array(live) }, returns: z.void() });
// @ts-expect-error a Principal cannot be an optional input
method({ input: { data: live.optional() }, returns: z.void() });
// @ts-expect-error a Principal cannot be returned
method({ input: {}, returns: live });
// @ts-expect-error a Caller cannot be returned inside an object
method({ input: {}, returns: z.object({ caller: liveCaller }) });
// @ts-expect-error a Principal cannot be part of the agent id
defineAgent({ name: 'Bad', id: { who: live }, methods: {} });

void PrincipalRef;
