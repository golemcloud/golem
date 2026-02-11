export interface Agent {
  accountId: string;
  args: string[];
  componentSize: number;
  createdAt: string;
  env: { [key: string]: string };
  lastError: string | null;
  exportedResourceInstances: { [key: string]: string };
  pendingInvocationCount: number;
  retryCount: number;
  status: string;
  totalLinearMemorySize: number;

  workerName: string;
  componentName: string;
  activePlugins: string[];
  updates: Update[];
  createdBy: string;
  environmentId: string;
  componentRevision: number;

  // "componentName": "pack:ts",
  // "workerName": "human-agent(\"bob\")",
  // "createdBy": "51de7d7d-f286-49aa-b79a-96022f7e2df9",
  // "environmentId": "019c0f30-b245-7291-a677-c3214dc18104",
  // "env": {},
  // "status": "Idle",
  // "componentRevision": 0,
  // "retryCount": 0,
  // "pendingInvocationCount": 0,
  // "updates": [],
  // "createdAt": "2026-01-30T14:48:17.301Z",
  // "lastError": null,
  // "componentSize": 5625981,
  // "totalLinearMemorySize": 4915200,
  // "exportedResourceInstances": {}
}

export interface Update {
  details?: string;
  targetVersion: number;
  timestamp: string;
  type: "failedUpdate" | "successfulUpdate";
}

export interface AgentStatus {
  Idle?: number;
  Running?: number;
  Suspended?: number;
  Failed?: number;
}

export interface Invocation {
  timestamp: string;
  function: string;
}

export interface Terminal {
  timestamp: string;
  message: string;
  bytes?: [];
}

interface BaseLogEntry {
  type: string;
  timestamp: string;
}

interface AgentId {
  componentId: string;
  agentName: string;
}

interface AttributeValue {
  type: string;
  value: string;
}

interface Attribute {
  key: string;
  value: AttributeValue;
}

interface LocalSpan {
  type: string;
  spanId: string;
  start: string;
  parentId: string | null;
  linkedContext: unknown | null;
  attributes: Attribute[];
  inherited: boolean;
}

interface CreateEntry extends BaseLogEntry {
  type: "Create";
  agentId: AgentId;
  componentRevision: number;
  args: unknown[];
  env: Record<string, unknown>;
  accountId: string;
  parent: string | null;
  componentSize: number;
  initialTotalLinearMemorySize: number;
  initialActivePlugins: unknown[];
}

interface ExportedFunctionInvokedEntry extends BaseLogEntry {
  type: "ExportedFunctionInvoked";
  functionName: string;
  request: unknown[];
  idempotencyKey: string;
  traceId: string;
  traceStates: unknown[];
  invocationContext: LocalSpan[][];
}

interface ResponseType {
  typ: {
    type: string;
    items?: { type: string }[];
  };
  value: unknown;
}

interface ExportedFunctionCompletedEntry extends BaseLogEntry {
  type: "ExportedFunctionCompleted";
  response: ResponseType;
  consumedFuel: number;
}

type OplogEntry =
  | CreateEntry
  | ExportedFunctionInvokedEntry
  | ExportedFunctionCompletedEntry;

export type OplogWithIndex = [number, OplogEntry];
