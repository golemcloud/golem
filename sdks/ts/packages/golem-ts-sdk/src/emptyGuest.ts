import type {
  golemAgent200Guest,
  golemTool010Guest,
  toolMiddlewareGuest as MiddlewareGuest,
  saveSnapshot as SaveSnapshot,
  loadSnapshot as LoadSnapshot,
} from 'agent-guest';
import { closeAsyncIterable } from './internal/tool/asyncIterable';

function invalidToolName(name: string): never {
  throw { tag: 'invalid-tool-name', val: name };
}

export const guest: typeof golemAgent200Guest = {
  discoverAgentTypes: () => [],
  async initialize(name) {
    throw { tag: 'invalid-type', val: name };
  },
  async invoke(name) {
    throw { tag: 'invalid-method', val: name };
  },
  getDefinition() {
    throw new Error('This component does not define agents');
  },
};

export const tool: typeof golemTool010Guest = {
  discoverTools: () => [],
  getTool: invalidToolName,
  async invoke(name, _path, _input, stdin) {
    // Release a supplied stream even when no tool can consume it.
    await closeAsyncIterable(stdin);
    return invalidToolName(name);
  },
};

export const toolMiddlewareGuest: typeof MiddlewareGuest = {
  discoverToolMiddlewares: () => [],
  getToolMiddleware: invalidToolName,
  async invokeToolMiddleware(name) {
    return invalidToolName(name);
  },
};

export const saveSnapshot: typeof SaveSnapshot = {
  async save() {
    throw new Error('This component does not define agents');
  },
};

export const loadSnapshot: typeof LoadSnapshot = {
  async load() {
    throw 'This component does not define agents';
  },
};
