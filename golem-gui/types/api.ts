import {
  AnalysedType,
  NameOptionTypePair,
  NameTypePair,
} from "./golem-data-types";

export interface VersionedComponentId {
  componentId: string;
  version: number;
}

export type ComponentExport = WorkerInstanceFunctions | WorkerFunction;

export interface ComponentMetadata {
  exports: ComponentExport[];
  producers: Producer[];
  memories: Memory[];
}

export interface WorkerFunction {
  name: string;
  parameters: Parameter[];
  results: Result[];
  value?: unknown;
  type: "Function";
}

export interface WorkerInstanceFunctions {
  type: "Instance";
  name: string;
  functions: WorkerFunction[];
  value?: unknown;
}
export type Parameter = (NameTypePair | NameOptionTypePair) & {
  value?: "";
};

export interface Result {
  name: string;
  typ: AnalysedType;
}
export interface Producer {
  fields: Array<{
    name: string;
    values: Array<{
      name: string;
      version: string;
    }>;
  }>;
}

export interface Memory {
  initial: number;
  maximum: number;
}

export interface ComponentFile {
  key: string;
  path: string;
  permissions: "read-only" | "read-write";
}

export interface InstalledPlugin {
  id: string;
  name: string;
  version: string;
  priority: number;
  parameters: Record<string, string>;
}

export interface Component {
  versionedComponentId: VersionedComponentId;
  componentName: string;
  componentSize: number;
  metadata: ComponentMetadata;
  createdAt: string;
  componentType: "Durable" | "Ephemeral";
  files: ComponentFile[];
  installedPlugins: InstalledPlugin[];
}

// Worker Types
export interface WorkerId {
  componentId: string;
  workerName: string;
}

export type WorkerStatus =
  | "Running"
  | "Idle"
  | "Suspended"
  | "Interrupted"
  | "Retrying"
  | "Failed"
  | "Exited";

export interface WorkerUpdate {
  type: "pendingUpdate";
  timestamp: string;
  targetVersion: number;
}

export type WorkerFormData = {
  name: string;
  args: string[];
  env: Record<string, string>;
};

export interface WorkerResource {
  createdAt: string;
  indexed: {
    resourceName: string;
    resourceParams: string[];
  };
}

export interface Worker {
  workerId: WorkerId;
  args: string[];
  env: Record<string, string>;
  status: WorkerStatus;
  componentVersion: number;
  retryCount: number;
  pendingInvocationCount: number;
  updates: WorkerUpdate[];
  createdAt: string;
  lastError?: string;
  componentSize: number;
  totalLinearMemorySize: number;
  ownedResources: Record<string, WorkerResource>;
  activePlugins: string[];
}

// API Definition Types
export interface ApiRoute {
  method: string;
  path: string;
  security?: string | null;
  binding: {
    componentId: VersionedComponentId;
    workerName: string;
    idempotencyKey?: string | null;
    response: string;
    bindingType: string;
    responseMappingInput?: Record<string, unknown>;
    workerNameInput?: Record<string, unknown>;
    idempotencyKeyInput?: Record<string, unknown> | null;
    corsPreflight?: {
      allowOrigin: string;
      allowMethods: string;
      allowHeaders: string;
      exposeHeaders: string;
      allowCredentials: boolean;
      maxAge: number;
    } | null;
    responseMappingOutput?: Record<string, unknown>;
  };
}

export interface ApiDefinition {
  id: string;
  version: string;
  routes: ApiRoute[];
  draft: boolean;
  createdAt: string;
}

export type DeploymentApiDefinition = {
  id: string;
  version: string;
};

type Site = {
  host: string;
  subdomain: string;
};

export type ApiDeployment = {
  apiDefinitions: DeploymentApiDefinition[];
  createdAt?: string;
  site: Site;
};

export interface OplogProcessorSpecs {
  type: "OplogProcessor"; // Fixed type for identification
  componentId: string; // ID of the component being processed
  componentVersion: string; // Version of the component
}

export interface ComponentTransformerSpecs {
  type: "ComponentTransformer"; // Fixed type for identification
  jsonSchema: string; // JSON schema definition as a string
  validateUrl: string; // URL for validation
  transformUrl: string; // URL for transformation
}

export interface Plugin {
  name: string;
  version: string;
  description?: string;
  icon: number[];
  homepage?: string;
  specs: OplogProcessorSpecs | ComponentTransformerSpecs;
  scope: {
    type: "Global";
  };
  owner?: Record<string, unknown>;
}
export interface GolemError {
  error?: string;
  errors?: string[];
  type?: string;
  golemError?: {
    type: string;
    details: string;
  };
}

export interface PluginInstallationDescription {
  installation_id: string; // UUID format
  plugin_name: string;
  plugin_version: string;
  parameters: Record<string, string>; // Object with string keys and string values
}

export interface CreateLogEntry {
  account_id: string;
  args: string[];
  component_size: number;
  component_version: number;
  env: Record<string, unknown>;
  initial_active_plugins: PluginInstallationDescription[];
  initial_total_linear_memory_size: number;
  parent: string | null;
  timestamp: string;
  type: "Create";
  worker_id: WorkerId;
}

interface ValueAndType {
  typ: AnalysedType; // The type of the value
  value: unknown; // The actual value (generic to accommodate various types)
}

interface ExportedFunctionParameters {
  function_name: string;
  idempotency_key: string; // Unique identifier
  full_function_name?: string; // Name of the function, including its scope or namespace
  function_input?: ValueAndType[]; // Array of function inputs, with value and type metadata
}

export interface ExportedFunctionInvokedEntry
  extends ExportedFunctionParameters {
  request: ValueAndType[]; // Array of requests, can hold any type
  timestamp: string; // ISO 8601 timestamp string
  type: "ExportedFunctionInvoked"; // Literal type for identification
}

export interface ExportedFunctionCompletedEntry {
  consumed_fuel: number;
  response: ValueAndType[]; // Array of requests, can hold any type
  type: "ExportedFunctionInvoked"; // Literal type for identification
}

export enum LogLevel {
  Stdout = "Stdout",
  Stderr = "Stderr",
  Trace = "Trace",
  Debug = "Debug",
  Info = "Info",
  Warn = "Warn",
  Error = "Error",
  Critical = "Critical",
}

export interface LogParameters {
  timestamp: string; // ISO 8601 date-time format
  level: LogLevel; // Enum defined above
  context: string; // Additional context for the log
  message: string; // Log message
}

export interface PublicOplogEntry_LogParameters extends LogParameters {
  type: "Log"; // Fixed value
}

export interface EntryWrapper {
  entry:
    | CreateLogEntry
    | ExportedFunctionInvokedEntry
    | ExportedFunctionCompletedEntry
    | PublicOplogEntry_LogParameters;
  oplogIndex: number;
}

export interface OpLog {
  entry: EntryWrapper;
}

export interface OplogQueryParams {
  from?: number;
  count: number;
  cursor?: string;
  query?: string;
}

export interface InstallPluginPayload {
  name: string;
  version: string;
  priority: number;
  parameters: Record<string, string>;
}

export interface UpdatePluginInstallPayload {
  priority: number;
  parameters: Record<string, string>;
}

export type InvocationStart = {
  InvocationStart: {
    timestamp: string; // ISO 8601 format date string
    function: string; // Function name with namespace or path
    idempotency_key: string; // Unique key for idempotency
  };
};

export type StdOutMessage = {
  StdOut: {
    timestamp: string; // ISO 8601 format date string
    bytes: number[]; // Array of bytes representing the message
  };
};

export type InvocationFinishedMessage = {
  InvocationFinished: {
    timestamp: string; // ISO 8601 format date string
    function: string; // Identifier of the function invoked
    idempotency_key: string; // Unique key for ensuring idempotency
  };
};

export type EventMessage =
  | InvocationStart
  | StdOutMessage
  | InvocationFinishedMessage;

export type WebSocketMessage = {
  type: string;
  data: EventMessage; // You can replace `any` with a more specific type if known
};
export type FilterComparator =
  | "Equal"
  | "Greater"
  | "GreaterEqual"
  | "LessEqual"
  | "Less"
  | "NotEqual";

export type StringFilterComparator = "Equal" | "NotEqual" | "Like" | "NotLike";

export type WorkerNormalFilter = {
  type: string;
  comparator: string;
  value: string | number | WorkerStatus;
  name?: string; // For Env filter
};

export interface WorkerHybridFilter {
  // type: "OR" | "AND" | "NOT";
  type: string;
  filters: WorkerNormalFilter[];
}

export type Cursor = {
  cursor: number;
  layer: number;
} | null;

export type WorkerFilter = {
  count?: number;
  cursor?: Cursor;
  filter?: {
    // type: "OR" | "AND" | "NOT";
    type: string;
    filters: (WorkerNormalFilter | WorkerHybridFilter)[];
  } | null;
  precise?: boolean;
};

export interface WorkerListResponse {
  workers: Worker[];
  cursor?: {
    cursor: number;
    layer: number;
  };
}
