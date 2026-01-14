import {
  HeaderVariable,
  HttpMountDetails,
  PathSegment,
  QueryVariable,
} from 'golem:agent/common';
import { AgentDecoratorOptions } from '../options';

export type HeaderVariables = Record<string, string>;

export function getHttpMountDetails(
  agentDecoratorOptions?: AgentDecoratorOptions,
): HttpMountDetails | undefined {
  if (!agentDecoratorOptions?.mount) return undefined;

  const [path, query] = splitPathAndQuery(agentDecoratorOptions.mount);

  const pathPrefix = parsePath(path);
  const queryVars = parseQuery(query);
  const headerVars = parseHeaderVars(agentDecoratorOptions.headers);

  return {
    pathPrefix,
    queryVars,
    headerVars,
    authDetails: agentDecoratorOptions.auth
      ? { required: true }
      : { required: false },
    phantomAgent: false,
    corsOptions: {
      allowedPatterns: agentDecoratorOptions.cors ?? [],
    },
    webhookSuffix: parseWebhook(agentDecoratorOptions.webhookSuffix),
  };
}

function parsePath(path: string): PathSegment[] {
  if (!path.startsWith('/')) {
    throw new Error(`HTTP mount must start with "/"`);
  }

  const segments = path.split('/').slice(1);

  return segments.map((segment) => {
    if (!segment) {
      throw new Error(`Empty path segment ("//") is not allowed`);
    }

    if (segment.startsWith('{') && segment.endsWith('}')) {
      const name = segment.slice(1, -1);

      if (!name) {
        throw new Error(`Empty path variable "{}" is not allowed`);
      }

      if (name === 'agent-type' || name === 'agent-version') {
        return {
          concat: [
            {
              tag: 'system-variable',
              val: name,
            },
          ],
        };
      }

      validateIdentifier(name);

      return {
        concat: [
          {
            tag: 'path-variable',
            val: { variableName: name },
          },
        ],
      };
    }

    validateLiteral(segment);

    return {
      concat: [
        {
          tag: 'literal',
          val: segment,
        },
      ],
    };
  });
}

function splitPathAndQuery(mount: string): [string, string | undefined] {
  const idx = mount.indexOf('?');
  return idx === -1
    ? [mount, undefined]
    : [mount.slice(0, idx), mount.slice(idx + 1)];
}

function parseWebhook(webhook?: string): PathSegment[] {
  if (!webhook) return [];
  return parsePath(webhook);
}

function parseHeaderVars(headers?: HeaderVariables): HeaderVariable[] {
  if (!headers) return [];

  return Object.entries(headers).map(([headerName, variableName]) => {
    validateIdentifier(variableName);

    return {
      headerName,
      variableName,
    };
  });
}

function parseQuery(query?: string): QueryVariable[] {
  if (!query) return [];

  return query.split('&').map((pair) => {
    const [key, value] = pair.split('=');

    if (!key || !value) {
      throw new Error(`Invalid query segment "${pair}"`);
    }

    if (!value.startsWith('{') || !value.endsWith('}')) {
      throw new Error(`Query value for "${key}" must be a variable reference`);
    }

    const variableName = value.slice(1, -1);
    validateIdentifier(variableName);

    return {
      queryParamName: key,
      variableName,
    };
  });
}

function validateIdentifier(name: string) {
  if (!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(name)) {
    throw new Error(`Invalid variable name "${name}"`);
  }
}

function validateLiteral(segment: string) {
  if (segment.includes('{') || segment.includes('}')) {
    throw new Error(`Invalid literal path segment "${segment}"`);
  }
}
