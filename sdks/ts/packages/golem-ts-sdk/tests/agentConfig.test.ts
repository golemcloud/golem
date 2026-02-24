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

import { describe, it } from 'vitest';
import { getHttpMountDetails } from '../src/internal/http/mount';
import { AgentClassName, AgentDecoratorOptions } from '../src';
import { parseQuery } from '../src/internal/http/query';
import { AgentMethod, HttpEndpointDetails, HttpMountDetails } from 'golem:agent/common';
import { validateHttpEndpoint, validateHttpMount } from '../src/internal/http/validation';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { tuple } from 'fast-check';

describe('agent config handling', () => {
  it('correctly describes a complex config type', () => {
    const configAgent = TypeMetadata.get("ConfigAgent")!;
    const constructorArgs = configAgent.constructorArgs;

    const arg = constructorArgs[0];
    expect(arg.name).toBe('config');
    expect(arg.type.optional).toBe(false);
    expect(arg.type.kind).toBe('config');
    assert(arg.type.kind === 'config');
    expect(arg.type.properties).toHaveLength(7)
    expect(arg.type.properties).toEqual(
      expect.arrayContaining([
        { path: ['foo'], secret: false, type: { kind: 'number', optional: false } },
        { path: ['bar'], secret: false, type: { kind: 'string', optional: false } },
        { path: ['secret'], secret: true, type: { kind: 'boolean', optional: false } },
        { path: ['nested', 'nestedSecret'], secret: true, type: { kind: 'number', optional: false } },
        { path: ['nested', 'a'], secret: false, type: { kind: 'boolean', optional: false } },
        { path: ['nested', 'b'], secret: false, type: { kind: 'array', element: { 'kind': 'number', optional: false }, optional: false } },
        { path: ['aliasedNested', 'c'], secret: false, type: { kind: 'number', optional: false } }
      ])
    );
  });

  it('correctly describes expected config entries to the host', () => {
    const configAgent = AgentTypeRegistry.get(new AgentClassName("ConfigAgent"))!;
    expect(configAgent.config).toHaveLength(7)
    expect(configAgent.config).toEqual(
      expect.arrayContaining([
         {
           "key": [
             "foo",
           ],
           "value": {
             "tag": "local",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-f64-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "bar",
           ],
           "value": {
             "tag": "local",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-string-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "secret",
           ],
           "value": {
             "tag": "shared",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-bool-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "nested",
             "nestedSecret",
           ],
           "value": {
             "tag": "shared",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-f64-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "nested",
             "a",
           ],
           "value": {
             "tag": "local",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-bool-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "nested",
             "b",
           ],
           "value": {
             "tag": "local",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "list-type",
                     "val": 1,
                   },
                 },
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-f64-type",
                   },
                 },
               ],
             },
           },
         },
         {
           "key": [
             "aliasedNested",
             "c",
           ],
           "value": {
             "tag": "local",
             "val": {
               "nodes": [
                 {
                   "name": undefined,
                   "owner": undefined,
                   "type": {
                     "tag": "prim-f64-type",
                   },
                 },
               ],
             },
           },
         },
      ])
    );
  });

  it('config parameters should not show up in declared constructor', () => {
    const configAgent = AgentTypeRegistry.get(new AgentClassName("ConfigAgent"))!;
    expect(configAgent.constructor.inputSchema).toEqual({ tag: 'tuple', val: [] })
  });
})
