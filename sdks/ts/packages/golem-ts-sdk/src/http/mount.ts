import {
  HeaderVariable,
  HttpMountDetails,
  PathSegment,
} from 'golem:agent/common';
import { AgentDecoratorOptions } from '../options';
import { parsePath } from './path';
import { parseQuery } from './query';
import { validateIdentifier } from './identifier';

export type HeaderVariables = Record<string, string>;

export function getHttpMountDetails(
  agentDecoratorOptions?: AgentDecoratorOptions,
): HttpMountDetails | undefined {
  if (!agentDecoratorOptions?.mount) return undefined;

  const [path, query] = splitPathAndQuery(agentDecoratorOptions.mount);

  const pathPrefix = parsePath(path);
  const queryVars = query ? parseQuery(query) : [];
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
