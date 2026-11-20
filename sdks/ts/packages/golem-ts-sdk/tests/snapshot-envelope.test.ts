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
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { schemaValueToWit, v } from '../src/internal/schema-model';
import { selectSelfAgent } from './selfAgentTestSupport';

const idInput = schemaValueToWit(v.record([]));
const anonymous = { tag: 'anonymous' };

async function setup() {
  vi.resetModules();
  const sdk = await import('../src');
  const loaded: Uint8Array[] = [];
  sdk
    .defineAgent({
      name: 'EnvelopeAgent',
      id: {},
      snapshotting: 'default',
      methods: { get: sdk.method({ input: {}, returns: z.number() }) },
    })
    .implement({
      init: () => ({ count: 0 }),
      methods: {
        get() {
          return this.count;
        },
      },
      snapshot: {
        save() {
          return new TextEncoder().encode(JSON.stringify({ count: this.count }));
        },
        load(bytes) {
          loaded.push(bytes);
          return JSON.parse(new TextDecoder().decode(bytes));
        },
      },
    });
  selectSelfAgent('EnvelopeAgent');
  return { sdk, loaded };
}

describe('the 2.0 snapshot envelope', () => {
  beforeEach(() => vi.resetModules());

  it('is a version-3 binary with no principal, and it loads back', async () => {
    const { sdk, loaded } = await setup();
    await sdk.loadSnapshot.load({
      payload: new Uint8Array([3, ...new TextEncoder().encode('{"count":4}')]),
      mimeType: 'application/octet-stream',
    }, idInput);
    const saved = await sdk.saveSnapshot.save();
    expect(saved.payload[0]).toBe(3);
    expect(new TextDecoder().decode(saved.payload.slice(1))).toBe('{"count":4}');
    expect(loaded).toHaveLength(1);
  });

  it('is a version-2 JSON object with no principal', async () => {
    const { sdk } = await setup();
    const text = (payload: Uint8Array) => new TextDecoder().decode(payload);
    expect(text((await setupJsonSave()).payload)).toBe(
      '{"version":2,"state":{"count":1},"fileDatabases":{}}',
    );

    async function setupJsonSave() {
      sdk
        .defineAgent({
          name: 'EnvelopeJson',
          id: {},
          snapshotting: { state: z.object({ count: z.number() }) },
          methods: {},
        })
        .implement({ init: () => ({ count: 1 }), methods: {} });
      selectSelfAgent('EnvelopeJson');
      await sdk.golemAgent200Guest.initialize('EnvelopeJson', idInput);
      return sdk.saveSnapshot.save();
    }
  });
});

describe('each 1.6 envelope version still loads and drops its principal', () => {
  beforeEach(() => vi.resetModules());

  it('reads binary v1 (no principal) and binary v2 (principal JSON)', async () => {
    const { sdk, loaded } = await setup();
    const state = new TextEncoder().encode('{"count":1}');
    await sdk.loadSnapshot.load({
      payload: new Uint8Array([1, ...state]),
      mimeType: 'application/octet-stream',
    }, idInput);
    expect(new TextDecoder().decode(loaded[0])).toBe('{"count":1}');

    const { sdk: second, loaded: secondLoaded } = await setup();
    const principalJson = new TextEncoder().encode(JSON.stringify(anonymous));
    const header = new Uint8Array(5);
    header[0] = 2;
    new DataView(header.buffer).setUint32(1, principalJson.length, false);
    await second.loadSnapshot.load({
      payload: new Uint8Array([...header, ...principalJson, ...state]),
      mimeType: 'application/octet-stream',
    }, idInput);
    expect(new TextDecoder().decode(secondLoaded[0])).toBe('{"count":1}');
  });

  it('refuses a malformed 1.6 principal', async () => {
    const { sdk } = await setup();
    const bad = new TextEncoder().encode(JSON.stringify({ tag: 'agent', val: { componentId: 1 } }));
    const header = new Uint8Array(5);
    header[0] = 2;
    new DataView(header.buffer).setUint32(1, bad.length, false);
    await expect(
      sdk.loadSnapshot.load({
        payload: new Uint8Array([...header, ...bad, 1]),
        mimeType: 'application/octet-stream',
      }, idInput),
    ).rejects.toBeDefined();
  });
});
