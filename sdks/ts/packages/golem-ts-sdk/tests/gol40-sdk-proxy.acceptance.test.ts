// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it } from 'vitest';
import { client } from '@golemcloud/golem-ts-sdk';
import { schemaValueFromWit, v } from '../src/internal/schema-model';
import {
  artifactDefinition,
  conformanceTransport,
  type ConformanceObservation,
} from './components/gol-40-rich-tool';

const request = {
  region: 'us-east-1' as const,
  trace: true,
  profile: 'release' as const,
  request: {
    source: 'src/main.wasm',
    labels: new Map([
      ['team', 'runtime'],
      ['tier', 'gold'],
    ]),
  },
  inputs: ['src/a.wasm', 'src/b.wat'],
  format: 'json' as const,
  tag: ['release', 'signed'],
  define: new Map([
    ['opt', 3n],
    ['workers', 2n],
  ]),
  color: 'never' as const,
  checksum: true,
  verbose: 2,
};

const expectedInput = v.record([
  v.enum(1),
  v.bool(true),
  v.enum(1),
  v.record([
    v.string('src/main.wasm'),
    v.map([
      { key: v.string('team'), value: v.string('runtime') },
      { key: v.string('tier'), value: v.string('gold') },
    ]),
  ]),
  v.list([v.path('src/a.wasm'), v.path('src/b.wat')]),
  v.enum(0),
  v.list([v.string('release'), v.string('signed')]),
  v.map([
    { key: v.string('opt'), value: v.s64(3n) },
    { key: v.string('workers'), value: v.s64(2n) },
  ]),
  v.enum(2),
  v.bool(true),
  v.u32(2),
]);

describe('GOL-40 TypeScript SDK proxy acceptance', () => {
  it('encodes the exact command path and asymmetric argument tuple on both client paths', async () => {
    const observations: ConformanceObservation[] = [];
    const transport = conformanceTransport(observations);
    const generated = client(artifactDefinition, { transport });
    const definitionOwned = artifactDefinition.client({ transport });

    await generated.render(request).collect();
    await definitionOwned.render(request).collect();

    expect(
      observations.map(({ path, input, stdout, stderr }) => ({
        path,
        input: schemaValueFromWit(input.value),
        stdout,
        stderr,
      })),
    ).toEqual([
      { path: ['render'], input: expectedInput, stdout: true, stderr: true },
      { path: ['render'], input: expectedInput, stdout: true, stderr: true },
    ]);
  });
});
