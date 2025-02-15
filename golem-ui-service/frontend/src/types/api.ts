// Component Types
export interface VersionedComponentId {
  componentId: string;
  version: number;
}

export interface ComponentMetadata {
  exports: ComponentExport[];
  producers: Producer[];
  memories: Memory[];
}

export interface ComponentExport {
  type: string;
  name: string;
  // parameters: Parameter[];
  functions: ExportFunction[];
  // results: Result[];
}

export interface ExportFunction {
  name: string;
  parameters: Parameter[];
  results: Result[];
}

export interface Parameter {
  name: string;
  typ: TypeDefinition;
}

export interface Result {
  name: string;
  typ: TypeDefinition;
}

export interface TypeDefinition {
  type: "Variant" | "Record" | "List" | "Option" | "f32" | "f64" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "String" | "Bool" | "Unit";
  cases: Array<{
    name: string;
    typ: Record<string, unknown>;
  }>;
  inner?: TypeDefinition;
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
  security?: string;
  binding: {
    componentId: VersionedComponentId;
    workerName: string;
    idempotencyKey?: string;
    response?: string;
    bindingType: "default" | "file-server" | "cors-preflight";
    responseMappingInput?: Record<string, unknown>;
    workerNameInput?: Record<string, unknown>;
    idempotencyKeyInput?: Record<string, unknown>;
    corsPreflight?: {
      allowOrigin: string;
      allowMethods: string;
      allowHeaders: string;
      exposeHeaders: string;
      allowCredentials: boolean;
      maxAge: number;
    };
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

// Plugin Types
export interface Plugin {
  name: string;
  version: string;
  description: string;
  icon: number[];
  homepage: string;
  specs: {
    type: "ComponentTransformer" | "OplogProcessor";
    providedWitPackage: string;
    jsonSchema: string;
    validateUrl: string;
    transformUrl: string;
    componentId: string;
    componentVersion: number;
  };
  scope: {
    type: "Global";
  };
  owner?: Record<string, unknown>;
}

export interface PluginInstall {
  name: string;
  version: string;
  priority: number;
  parameters: Record<string, string>;
  id: string;
}
