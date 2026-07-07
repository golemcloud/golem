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

// NOTE (fluent port): the fluent `s.quotaToken()` marker encodes/decodes the RAW
// `golem:core/types` quota-token resource handle, and the SDK `QuotaToken`
// wrapper class (`acquireQuotaToken` / `token.reserve()`) does NOT interoperate
// with that marker through the public API. So this file uses the raw
// `golem:quota/types@1.5.0` host functions directly — which are exactly the
// handles `s.quotaToken()` carries — to keep the RPC-passing behavior faithful.

import { z } from 'zod';
import { defineAgent, method, s, clientFor } from '@golemcloud/golem-ts-sdk';
import {
  newToken,
  reserve,
  split,
  Reservation,
  QuotaToken as RawQuotaToken,
} from 'golem:quota/types@1.5.0';

export const QuotaRpcReceiver = defineAgent({
  name: 'QuotaRpcReceiver',
  id: { _name: z.string() },
  methods: {
    reserveAndCallInLoop: method({
      input: {
        childToken: s.quotaToken(),
        host: z.string(),
        port: z.number(),
        maxIterations: z.number(),
      },
      returns: z.void(),
    }),
  },
});

export const QuotaRpcReceiverImpl = QuotaRpcReceiver.implement({
  init: () => ({}),
  methods: {
    async reserveAndCallInLoop({ childToken, host, port, maxIterations }) {
      const token = childToken as RawQuotaToken;
      for (let i = 0; i < maxIterations; i++) {
        let reservation;
        try {
          reservation = reserve(token, 1n);
        } catch {
          break;
        }
        await fetch(`http://${host}:${port}/call`);
        Reservation.commit(reservation, 1n);
      }
    },
  },
});

const receiverClient = clientFor(QuotaRpcReceiver);

export const QuotaRpcSender = defineAgent({
  name: 'QuotaRpcSender',
  id: { name: z.string() },
  methods: {
    splitAndLoop: method({
      input: {
        resourceName: z.string(),
        totalExpectedUse: s.u64(),
        childExpectedUse: s.u64(),
        host: z.string(),
        port: z.number(),
        maxIterations: z.number(),
      },
      returns: z.void(),
    }),
  },
});

export const QuotaRpcSenderImpl = QuotaRpcSender.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    async splitAndLoop({ resourceName, totalExpectedUse, childExpectedUse, host, port, maxIterations }) {
      const token = newToken(resourceName, BigInt(totalExpectedUse));
      const childToken = split(token, BigInt(childExpectedUse));

      const receiverName = `${this.name}-receiver`;
      receiverClient({ _name: receiverName }).reserveAndCallInLoop.trigger({
        childToken,
        host,
        port,
        maxIterations,
      });

      for (let i = 0; i < maxIterations; i++) {
        let reservation;
        try {
          reservation = reserve(token, 1n);
        } catch {
          break;
        }
        await fetch(`http://${host}:${port}/call`);
        Reservation.commit(reservation, 1n);
      }
    },
  },
});
