import type * as Guest from "agent-guest"

export const golemAgent200Guest = {
  discoverAgentTypes: () => [],
  getDefinition: () => {
    throw new Error("agent is not initialized; cannot get definition")
  },
  initialize: async (name: string) => {
    throw new Error(`unknown agent type: ${name}`)
  },
  invoke: async (name: string) => {
    throw new Error(`agent is not initialized; cannot invoke ${name}`)
  },
} satisfies typeof Guest.golemAgent200Guest

export const saveSnapshot = {
  save: async () => {
    throw new Error("agent is not initialized; cannot save snapshot")
  },
} satisfies typeof Guest.saveSnapshot

export const loadSnapshot = {
  load: async () => {
    throw new Error("no agent is registered; cannot load snapshot")
  },
} satisfies typeof Guest.loadSnapshot

export const golemTool010Guest = {
  discoverTools: () => [],
  getTool: (name: string) => {
    throw { tag: "invalid-tool-name", val: name }
  },
  invoke: async (name: string) => {
    throw { tag: "invalid-tool-name", val: name }
  },
} satisfies typeof Guest.golemTool010Guest

export const toolMiddlewareGuest = {
  discoverToolMiddlewares: () => [],
  getToolMiddleware: (name: string) => {
    throw { tag: "invalid-tool-name", val: name }
  },
  invokeToolMiddleware: async (name: string) => {
    throw { tag: "invalid-tool-name", val: name }
  },
} satisfies typeof Guest.toolMiddlewareGuest
