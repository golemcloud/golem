export interface Worker {
  accountId: string;
  args: string[];
  componentSize: number;
  componentVersion: number;
  createdAt: string;
  env: { [key: string]: string };
  lastError: string | null;
  ownedResources: { [key: string]: string };
  pendingInvocationCount: number;
  retryCount: number;
  status: string;
  totalLinearMemorySize: number;

  workerName: string;
  componentName: string;
  activePlugins: string[];
  updates: Update[];
}

export interface Update {
  details?: string;
  targetVersion: number;
  timestamp: string;
  type: "failedUpdate" | "successfulUpdate";
}

export interface WorkerStatus {
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

export interface WsMessage {
  InvocationStart: Invocation;
  StdOut: Terminal;
}

interface BaseLogEntry {
  type: string;
  timestamp: string;
}

interface WorkerId {
  componentId: string;
  workerName: string;
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
  workerId: WorkerId;
  componentVersion: number;
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
