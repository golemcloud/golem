import { describe } from 'vitest';
import { AgentType } from 'golem:agent/common@1.5.0';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import {
  AllHttpMethodsAgentClassName,
  ComplexHttpAgentClassName,
  SimpleHttpAgentClassName,
} from './testUtils';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

describe('Http Agent class', () => {
  it('should register HTTP mount details with only mount', () => {
    const simpleHttpAgent = AgentTypeRegistry.get(SimpleHttpAgentClassName);

    if (!simpleHttpAgent) {
      throw new Error('SimpleHttpAgent not found in AgentTypeRegistry');
    }

    expect(simpleHttpAgent.httpMount).toBeDefined();
    expect(simpleHttpAgent.httpMount?.pathPrefix).toEqual([
      {
        tag: 'literal',
        val: 'chats',
      },
      {
        tag: 'system-variable',
        val: 'agent-type',
      },
    ]);
  });

  it('should register HTTP endpoint details with endpoint details', () => {
    const simpleHttpAgent = AgentMethodRegistry.get(SimpleHttpAgentClassName.value)?.get('greet');

    if (!simpleHttpAgent) {
      throw new Error('SimpleHttpAgent.greet method not found in AgentMethodRegistry');
    }

    expect(simpleHttpAgent.httpEndpoint).toBeDefined();
    expect(simpleHttpAgent.httpEndpoint).toEqual([
      {
        httpMethod: { tag: 'get' },
        authDetails: undefined,
        queryVars: [],
        corsOptions: {
          allowedPatterns: [],
        },
        headerVars: [],
        pathSuffix: [
          {
            tag: 'literal',
            val: 'greet',
          },
          {
            tag: 'path-variable',
            val: {
              variableName: 'name',
            },
          },
        ],
      },
    ]);
  });

  it('should register HTTP mount details with all details', () => {
    const simpleHttpAgent = AgentTypeRegistry.get(ComplexHttpAgentClassName);

    if (!simpleHttpAgent) {
      throw new Error('SimpleHttpAgent not found in AgentTypeRegistry');
    }

    const expectedPathPrefix = [
      {
        tag: 'literal',
        val: 'chats',
      },
      {
        tag: 'system-variable',
        val: 'agent-type',
      },
      {
        tag: 'path-variable',
        val: {
          variableName: 'foo',
        },
      },
      {
        tag: 'path-variable',
        val: {
          variableName: 'bar',
        },
      },
    ];

    const expectedWebhookSuffix = [
      {
        tag: 'system-variable',
        val: 'agent-type',
      },
      {
        tag: 'literal',
        val: 'events',
      },
      {
        tag: 'path-variable',
        val: {
          variableName: 'foo',
        },
      },
      {
        tag: 'path-variable',
        val: {
          variableName: 'bar',
        },
      },
    ];

    expect(simpleHttpAgent.httpMount).toBeDefined();
    expect(simpleHttpAgent.httpMount).toEqual({
      pathPrefix: expectedPathPrefix,
      authDetails: { required: true },
      phantomAgent: true,
      corsOptions: {
        allowedPatterns: ['https://app.acme.com', 'https://staging.acme.com'],
      },
      webhookSuffix: expectedWebhookSuffix,
    });
  });

  it('should register simple HTTP endpoint details with catch all var', () => {
    const complexHttpAgent = AgentMethodRegistry.get(ComplexHttpAgentClassName.value)?.get(
      'catchAllFun',
    );

    if (!complexHttpAgent) {
      throw new Error('ComplexHttpAgent.catchAllFun method not found in AgentMethodRegistry');
    }

    expect(complexHttpAgent.httpEndpoint).toBeDefined();
    expect(complexHttpAgent.httpEndpoint).toEqual([
      {
        httpMethod: { tag: 'get' },
        authDetails: undefined,
        queryVars: [],
        corsOptions: {
          allowedPatterns: [],
        },
        headerVars: [],
        pathSuffix: [
          { tag: 'literal', val: 'greet' },
          { tag: 'path-variable', val: { variableName: 'name' } },
          { tag: 'remaining-path-variable', val: { variableName: 'filePath' } },
        ],
      },
    ]);
  });

  it('should register simple HTTP endpoint details with left over parameters in request body', () => {
    const complexHttpAgent = AgentMethodRegistry.get(ComplexHttpAgentClassName.value)?.get(
      'greetPost',
    );

    if (!complexHttpAgent) {
      throw new Error('ComplexHttpAgent.greetPost method not found in AgentMethodRegistry');
    }

    expect(complexHttpAgent.httpEndpoint).toBeDefined();
    expect(complexHttpAgent.httpEndpoint).toEqual([
      {
        httpMethod: { tag: 'post' },
        authDetails: undefined,
        queryVars: [
          {
            queryParamName: 'l',
            variableName: 'location',
          },
        ],
        corsOptions: {
          allowedPatterns: [],
        },
        headerVars: [],
        pathSuffix: [
          {
            tag: 'literal',
            val: 'greet',
          },
        ],
      },
    ]);
  });

  it('should register complex HTTP endpoint details with endpoint details', () => {
    const complexHttpAgentMetadata = AgentMethodRegistry.get(ComplexHttpAgentClassName.value)?.get(
      'greetCustom',
    );

    if (!complexHttpAgentMetadata) {
      throw new Error('SimpleHttpAgent.greet method not found in AgentMethodRegistry');
    }

    expect(complexHttpAgentMetadata.httpEndpoint).toBeDefined();
    expect(complexHttpAgentMetadata.httpEndpoint).toEqual([
      {
        httpMethod: { tag: 'custom', val: 'patch' },
        authDetails: undefined,
        queryVars: [
          {
            queryParamName: 'l',
            variableName: 'location',
          },
          {
            queryParamName: 'n',
            variableName: 'name',
          },
        ],
        corsOptions: {
          allowedPatterns: [],
        },
        headerVars: [],
        pathSuffix: [
          {
            tag: 'literal',
            val: 'greet',
          },
        ],
      },
      {
        httpMethod: { tag: 'get' },
        authDetails: { required: true },
        queryVars: [
          {
            queryParamName: 'lx',
            variableName: 'location',
          },
          {
            queryParamName: 'nm',
            variableName: 'name',
          },
        ],
        corsOptions: {
          allowedPatterns: ['*'],
        },
        headerVars: [
          {
            headerName: 'X-Foo',
            variableName: 'location',
          },
          {
            headerName: 'X-Bar',
            variableName: 'name',
          },
        ],
        pathSuffix: [
          {
            tag: 'literal',
            val: 'greet',
          },
        ],
      },
      {
        httpMethod: { tag: 'get' },
        authDetails: undefined,
        queryVars: [
          {
            queryParamName: 'l',
            variableName: 'location',
          },
          {
            queryParamName: 'n',
            variableName: 'name',
          },
        ],
        corsOptions: {
          allowedPatterns: [],
        },
        headerVars: [],
        pathSuffix: [
          {
            tag: 'literal',
            val: 'greet',
          },
        ],
      },
    ]);
  });

  it('should register all HTTP methods correctly', () => {
    const allHttpMethodsAgent = AgentTypeRegistry.get(AllHttpMethodsAgentClassName);

    if (!allHttpMethodsAgent) {
      throw new Error('AllHttpMethodsAgent not found in AgentTypeRegistry');
    }

    const expectedMethods = [
      { name: 'getMethod', tag: 'get' },
      { name: 'postMethod', tag: 'post' },
      { name: 'putMethod', tag: 'put' },
      { name: 'deleteMethod', tag: 'delete' },
      { name: 'patchMethod', tag: 'patch' },
    ];

    for (const { name, tag } of expectedMethods) {
      const method = AgentMethodRegistry.get(AllHttpMethodsAgentClassName.value)?.get(name);

      if (!method) {
        throw new Error(`Method ${name} not found in AgentMethodRegistry`);
      }

      expect(method.httpEndpoint).toBeDefined();
      expect(method.httpEndpoint).toHaveLength(1);
      expect(method.httpEndpoint![0].httpMethod).toEqual({ tag });
      expect(method.httpEndpoint![0].authDetails).toBeUndefined();
      expect(method.httpEndpoint![0].queryVars).toEqual([]);
      expect(method.httpEndpoint![0].corsOptions).toEqual({ allowedPatterns: [] });
      expect(method.httpEndpoint![0].headerVars).toEqual([]);
    }
  });
});
