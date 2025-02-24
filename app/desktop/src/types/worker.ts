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
  workerId: {
    componentId: string;
    workerName: string;
  };
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

export interface OplogEntry {
  entry: {
    timestamp: string;
    message: string;
    function_name: string;
    type: "Log" | "ExportedFunctionInvoked";
  };
}
