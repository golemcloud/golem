// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { AgentClassName } from '../../agentClassName';
import { compileRouterMount } from '../../httpRouterContract';

interface NodeHttpRouter {
  readonly name: string;
  readonly mount: string;
  readonly auth?: boolean;
  readonly cors?: readonly string[];
}

let phase: 'unconfigured' | 'initializing' | 'closed' = 'unconfigured';
const routers = new Map<number, NodeHttpRouter>();
const registeredPorts = new Set<number>();

/** Called only by the generated component entry, before application evaluation. */
export function configureNodeHttpRegistration(table: unknown): void {
  if (phase !== 'unconfigured') throw new Error('Node HTTP registration cannot be reopened');
  if (!table || typeof table !== 'object' || Array.isArray(table))
    throw new TypeError('nodeHttpRouters must be a port-to-router object');
  const names = new Set<string>();
  for (const [key, value] of Object.entries(table)) {
    const port = Number(key);
    if (!Number.isInteger(port) || port < 1 || port > 65535 || String(port) !== key)
      throw new TypeError('nodeHttpRouters keys must be canonical decimal ports from 1 to 65535');
    if (
      !value ||
      typeof value !== 'object' ||
      Array.isArray(value) ||
      Object.keys(value).some((field) => !['name', 'mount', 'auth', 'cors'].includes(field)) ||
      typeof value.name !== 'string' ||
      typeof value.mount !== 'string' ||
      (value.auth !== undefined && typeof value.auth !== 'boolean') ||
      (value.cors !== undefined &&
        (!Array.isArray(value.cors) ||
          value.cors.some((origin: unknown) => typeof origin !== 'string')))
    )
      throw new TypeError(
        'nodeHttpRouters entries require name/mount and optional boolean auth/string[] cors',
      );
    const name = new AgentClassName(value.name).value;
    if (names.has(name)) throw new Error(`nodeHttpRouters repeats router name "${name}"`);
    names.add(name);
    compileRouterMount(value);
    routers.set(
      port,
      Object.freeze({
        name,
        mount: value.mount,
        auth: value.auth,
        cors: value.cors === undefined ? undefined : Object.freeze([...value.cors]),
      }),
    );
  }
  phase = 'initializing';
}

/** Synchronous and nonthrowing, including when configuration or application evaluation failed. */
export function closeNodeHttpRegistration(): void {
  phase = 'closed';
}

export function nodeHttpRouterForPort(port: number): NodeHttpRouter {
  if (phase === 'unconfigured')
    throw new Error(
      'Configure nodeHttpRouters for listen(), or use implementRaw(nodeHttpHandler(server))',
    );
  if (phase !== 'initializing')
    throw new Error('listen() registration is only allowed during component initialization');
  const router = routers.get(port);
  if (!router) throw new Error(`No nodeHttpRouters mapping for listen(${port})`);
  if (registeredPorts.has(port)) throw new Error(`listen(${port}) is already registered`);
  return router;
}

export function completeNodeHttpRegistration(port: number): void {
  registeredPorts.add(port);
}
