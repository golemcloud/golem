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
import { getHttpMountDetails } from '../src/http/mount';
import { AgentDecoratorOptions } from '../src';
import { parseQuery } from '../src/http/query';
import { HttpMountDetails } from 'golem:agent/common';
import { validateHttpMountWithConstructor } from '../src/http/validation';

describe('getHttpMountDetails – basic behavior', () => {
  it('returns undefined when mount is not provided', () => {
    const result = getHttpMountDetails({});
    expect(result).toBeUndefined();
  });

  it('parses a simple literal path', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/chats',
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.pathPrefix).toEqual([
      { concat: [{ tag: 'literal', val: 'chats' }] },
    ]);

    expect(result.queryVars).toEqual([]);
    expect(result.headerVars).toEqual([]);
    expect(result.authDetails).toEqual({ required: false });
    expect(result.phantomAgent).toBe(false);
    expect(result.corsOptions.allowedPatterns).toEqual([]);
    expect(result.webhookSuffix).toEqual([]);
  });
});

describe('getHttpMountDetails – path variables', () => {
  it('parses system and user path variables', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/v{agent-version}/chats/{chatId}',
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.pathPrefix).toEqual([
      {
        concat: [
          { tag: 'literal', val: 'v' },
          { tag: 'system-variable', val: 'agent-version' },
        ],
      },
      {
        concat: [{ tag: 'literal', val: 'chats' }],
      },
      {
        concat: [
          {
            tag: 'path-variable',
            val: { variableName: 'chatId' },
          },
        ],
      },
    ]);
  });

  it('parses mixed path segment with variable and literal', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/bar/{baz}abc',
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.pathPrefix).toEqual([
      {
        concat: [{ tag: 'literal', val: 'bar' }],
      },
      {
        concat: [
          { tag: 'path-variable', val: { variableName: 'baz' } },
          { tag: 'literal', val: 'abc' },
        ],
      },
    ]);
  });
});

describe('getHttpMountDetails – header variables', () => {
  it('parses header variables', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/chats',
      headers: {
        'X-Request-Id': 'requestId',
        'X-Tenant': 'tenant',
      },
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.headerVars).toEqual([
      { headerName: 'X-Request-Id', variableName: 'requestId' },
      { headerName: 'X-Tenant', variableName: 'tenant' },
    ]);
  });
});

describe('getHttpMountDetails – auth and cors', () => {
  it('sets auth and cors options', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/secure',
      auth: true,
      cors: ['https://app.acme.com'],
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.authDetails).toEqual({ required: true });
    expect(result.corsOptions.allowedPatterns).toEqual([
      'https://app.acme.com',
    ]);
  });
});

describe('getHttpMountDetails – webhook suffix', () => {
  it('parses webhook suffix path', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/chats/{id}',
      webhookSuffix: '/webhook/{event}',
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.webhookSuffix).toEqual([
      { concat: [{ tag: 'literal', val: 'webhook' }] },
      {
        concat: [
          {
            tag: 'path-variable',
            val: { variableName: 'event' },
          },
        ],
      },
    ]);
  });
});

describe('getHttpMountDetails – validation errors', () => {
  it('rejects mount with query parameters', () => {
    expect(() => getHttpMountDetails({ mount: '/chats?id={chatId}' })).toThrow(
      'HTTP mount must not contain query parameters',
    );
  });

  it('rejects webhook suffix with query parameters', () => {
    expect(() =>
      getHttpMountDetails({
        mount: '/chats',
        webhookSuffix: '/hook?event={event}',
      }),
    ).toThrow('HTTP webhook suffix must not contain query parameters');
  });

  it('rejects mount without leading slash', () => {
    expect(() => getHttpMountDetails({ mount: 'chats' })).toThrow(
      'HTTP mount must start with "/"',
    );
  });

  it('rejects empty path segments', () => {
    expect(() => getHttpMountDetails({ mount: '/chats//foo' })).toThrow(
      'Empty path segment',
    );
  });

  it('rejects unclosed path variable', () => {
    expect(() => getHttpMountDetails({ mount: '/chats/{id' })).toThrow(
      'Unclosed "{"',
    );
  });
});

describe('parseQuery', () => {
  it('parses query variables', () => {
    const result = parseQuery('foo={bar}&limit={limit}');

    expect(result).toEqual([
      { queryParamName: 'foo', variableName: 'bar' },
      { queryParamName: 'limit', variableName: 'limit' },
    ]);
  });
});

describe('validateHttpMountWithConstructor', () => {
  function mount(
    pathVars: string[] = [],
    headerVars: string[] = [],
  ): HttpMountDetails {
    return {
      pathPrefix: pathVars.map((v) => ({
        concat: [{ tag: 'path-variable', val: { variableName: v } }],
      })),
      headerVars: headerVars.map((v) => ({
        headerName: `X-${v}`,
        variableName: v,
      })),
      queryVars: [],
      authDetails: { required: false },
      phantomAgent: false,
      corsOptions: { allowedPatterns: [] },
      webhookSuffix: [],
    };
  }

  function constructorVars(...names: string[]) {
    return {
      inputSchema: {
        val: names.map((n) => [n, {}]),
      },
    } as any;
  }

  it('passes when all constructor variables are provided via path variables', () => {
    const agentMount = mount(['chatId', 'userId']);
    const agentConstructor = constructorVars('chatId', 'userId');

    expect(() =>
      validateHttpMountWithConstructor(agentMount, agentConstructor),
    ).not.toThrow();
  });

  it('passes when all constructor variables are provided via header variables', () => {
    const agentMount = mount([], ['tenant', 'requestId']);
    const agentConstructor = constructorVars('tenant', 'requestId');

    expect(() =>
      validateHttpMountWithConstructor(agentMount, agentConstructor),
    ).not.toThrow();
  });

  it('passes when constructor variables are split across path and headers', () => {
    const agentMount = mount(['chatId'], ['tenant']);
    const agentConstructor = constructorVars('chatId', 'tenant');

    expect(() =>
      validateHttpMountWithConstructor(agentMount, agentConstructor),
    ).not.toThrow();
  });

  it('fails when a constructor variable is not provided by the mount', () => {
    const agentMount = mount(['chatId'], ['tenant']);
    const agentConstructor = constructorVars('chatId', 'tenant', 'userId');

    expect(() =>
      validateHttpMountWithConstructor(agentMount, agentConstructor),
    ).toThrow(
      'Agent constructor variable "userId" is not provided by the HTTP mount (path or headers).',
    );
  });

  it('passes when the constructor has no variables', () => {
    const agentMount = mount(['chatId'], ['tenant']);
    const agentConstructor = constructorVars();

    expect(() =>
      validateHttpMountWithConstructor(agentMount, agentConstructor),
    ).not.toThrow();
  });
});
