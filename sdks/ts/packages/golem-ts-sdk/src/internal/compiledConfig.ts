// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { getConfigValue, type TypedAgentConfigValue } from 'golem:agent/host@2.0.0';
import type { SchemaGraph, Secret as RawSecret } from 'golem:core/types@2.0.0';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import { Secret } from '../secret';
import { readConcrete, writeConcrete, type ConcreteCodec } from './tool/compiled';

export type ConfigNode =
  | {
      name: string;
      children: ConfigNode[];
      requiredKeys: string[];
    }
  | {
      name: string;
      path: string[];
      graph: SchemaGraph;
      codec: ConcreteCodec;
      secret?: { codec: ConcreteCodec; graph: SchemaGraph };
    };

export function compiledConfig(nodes: ConfigNode[]) {
  const read = (node: ConfigNode): unknown => {
    if ('children' in node) {
      const value = Object.fromEntries(node.children.map((child) => [child.name, read(child)]));
      return node.requiredKeys.some((key) => value[key] === undefined) ? undefined : value;
    }
    if (node.secret) {
      const withHandle = <R>(use: (handle: RawSecret) => R): R => {
        const raw = readConcrete(node.codec, getConfigValue(node.path, node.graph)) as RawSecret;
        try {
          return use(raw);
        } finally {
          (raw as RawSecret & { [Symbol.dispose]?: () => void })[Symbol.dispose]?.();
        }
      };
      return new Secret(
        () =>
          withHandle((raw) => readConcrete(node.secret!.codec, reveal(raw, node.secret!.graph))),
        withHandle,
      );
    }
    return readConcrete(node.codec, getConfigValue(node.path, node.graph));
  };
  return {
    accessor() {
      const value: Record<string, unknown> = {};
      for (const node of nodes)
        Object.defineProperty(value, node.name, {
          enumerable: true,
          configurable: true,
          get: () => read(node),
        });
      return value;
    },
    overrides(value: Record<string, unknown> = {}): TypedAgentConfigValue[] {
      const result: TypedAgentConfigValue[] = [];
      const visit = (node: ConfigNode) => {
        if ('children' in node) return node.children.forEach(visit);
        let found: unknown = value;
        for (const segment of node.path) {
          if (typeof found !== 'object' || found === null)
            throw new TypeError(`Expected config object at ${node.path.join('.')}`);
          if (!Object.prototype.hasOwnProperty.call(found, segment)) return;
          found = (found as Record<string, unknown>)[segment];
        }
        if (node.secret)
          throw new TypeError(
            `Cannot override secret config field '${node.path.join('.')}' over RPC`,
          );
        result.push({
          path: node.path,
          value: { graph: node.graph, value: writeConcrete(node.codec, found) },
        });
      };
      nodes.forEach(visit);
      return result;
    },
  };
}
