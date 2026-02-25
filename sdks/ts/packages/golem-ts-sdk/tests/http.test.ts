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
import { AgentDecoratorOptions } from '../src';
import { parseQuery } from '../src/internal/http/query';
import { AgentMethod, HttpEndpointDetails, HttpMountDetails } from 'golem:agent/common@1.5.0';
import { validateHttpEndpoint, validateHttpMount } from '../src/internal/http/validation';

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

    expect(result.pathPrefix).toEqual([{ tag: 'literal', val: 'chats' }]);

    expect(result.authDetails).toEqual({ required: false });
    expect(result.phantomAgent).toBe(false);
    expect(result.corsOptions.allowedPatterns).toEqual([]);
    expect(result.webhookSuffix).toEqual([]);
  });
});

describe('getHttpMountDetails – path variables', () => {
  it('parses system and user path variables', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/{agent-version}/chats/{chatId}',
    };

    const result = getHttpMountDetails(opts)!;

    expect(result.pathPrefix).toEqual([
      { tag: 'system-variable', val: 'agent-version' },
      { tag: 'literal', val: 'chats' },
      {
        tag: 'path-variable',
        val: { variableName: 'chatId' },
      },
    ]);
  });
});

describe('getHttpMountDetails – header variables', () => {
  it('parses header variables', () => {
    const opts: AgentDecoratorOptions = {
      mount: '/chats',
    };

    const result = getHttpMountDetails(opts)!;
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
    expect(result.corsOptions.allowedPatterns).toEqual(['https://app.acme.com']);
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
      { tag: 'literal', val: 'webhook' },
      {
        tag: 'path-variable',
        val: { variableName: 'event' },
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
    expect(() => getHttpMountDetails({ mount: 'chats' })).toThrow('HTTP mount must start with "/"');
  });

  it('rejects empty path segments', () => {
    expect(() => getHttpMountDetails({ mount: '/chats//foo' })).toThrow('Empty path segment');
  });

  it('rejects unclosed path variable', () => {
    expect(() => getHttpMountDetails({ mount: '/chats/{id' })).toThrow(
      'Path segment "{id" must be a whole variable like "{id}" and cannot mix literals and variables',
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
  function mount(pathVars: string[] = []): HttpMountDetails {
    return {
      pathPrefix: pathVars.map((v) => ({
        tag: 'path-variable',
        val: { variableName: v },
      })),
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

    expect(() => validateHttpMount('Foo', agentMount, agentConstructor)).not.toThrow();
  });

  it('fails when a constructor variable is not provided by the mount', () => {
    const agentMount = mount(['chatId']);
    const agentConstructor = constructorVars('chatId', 'tenant');

    expect(() => validateHttpMount('Foo', agentMount, agentConstructor)).toThrow(
      "Agent constructor variable 'tenant' is not provided by the HTTP mount path.",
    );
  });

  it('fails when the mount refers to the path variables that are not part of the constructor', () => {
    const agentMount = mount(['chatId']);
    const agentConstructor = constructorVars();

    expect(() => validateHttpMount('Foo', agentMount, agentConstructor)).toThrow(
      "HTTP mount path variable 'chatId' (in path segment 0) is not defined in the agent constructor.",
    );
  });
});

describe('validateHttpEndpoint', () => {
  function endpoint(
    pathVars: string[] = [],
    queryVars: string[] = [],
    headerVars: string[] = [],
  ): HttpEndpointDetails {
    return {
      pathSuffix: pathVars.map((v) => ({
        tag: 'path-variable',
        val: { variableName: v },
      })),
      queryVars: queryVars.map((v) => ({
        queryParamName: v,
        variableName: v,
      })),
      headerVars: headerVars.map((v) => ({
        headerName: `X-${v}`,
        variableName: v,
      })),
    } as any;
  }

  function method(vars: string[], endpoints: HttpEndpointDetails[]): AgentMethod {
    return {
      name: 'doThing',
      description: '',
      promptHint: '',
      inputSchema: {
        tag: 'tuple',
        val: vars.map((v) => [v, { tag: 'component-model', val: { nodes: [] } }]),
      },
      outputSchema: { tag: 'tuple', val: [] },
      httpEndpoint: endpoints,
    };
  }

  const agentName = 'TestAgent';

  const httpMountDetails: HttpMountDetails = {
    pathPrefix: [{ tag: 'literal', val: 'test' }],
    authDetails: { required: false },
    phantomAgent: false,
    corsOptions: { allowedPatterns: [] },
    webhookSuffix: [],
  };

  it('passes when all method parameters are provided via path', () => {
    const agentMethod = method(['id', 'event'], [endpoint(['id', 'event'])]);

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).not.toThrow();
  });

  it('passes when method parameters are split across path, query, and headers', () => {
    const agentMethod = method(
      ['id', 'limit', 'tenant'],
      [endpoint(['id'], ['limit'], ['tenant'])],
    );

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).not.toThrow();
  });

  it('fails when endpoint path variable is not part of method input', () => {
    const agentMethod = method([], [endpoint(['id'])]);

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).toThrow(
      "HTTP endpoint path variable 'id' is not defined in method input parameters.",
    );
  });

  it('fails when endpoint query variable is not part of method input', () => {
    const agentMethod = method([], [endpoint([], ['limit'])]);

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).toThrow(
      "HTTP endpoint query variable 'limit' is not defined in method input parameters.",
    );
  });

  it('fails when endpoint header variable is not part of method input', () => {
    const agentMethod = method([], [endpoint([], [], ['tenant'])]);

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).toThrow(
      "HTTP endpoint header variable 'tenant' is not defined in method input parameters.",
    );
  });

  it('validates each endpoint independently', () => {
    const agentMethod = method(
      ['id'],
      [
        endpoint(['id']),
        endpoint(['foo']), // invalid second endpoint
      ],
    );

    expect(() => validateHttpEndpoint(agentName, agentMethod, httpMountDetails)).toThrow(
      "HTTP endpoint path variable 'foo' is not defined in method input parameters.",
    );
  });

  it('endpoints with no mount details', () => {
    const agentMethod = method(['id'], [endpoint(['id'])]);

    expect(() => validateHttpEndpoint(agentName, agentMethod, undefined)).toThrow(
      "Agent method 'doThing' of 'TestAgent' defines HTTP endpoints but the agent is not mounted over HTTP. Please specify mount details in 'agent' decorator.",
    );
  });
});
