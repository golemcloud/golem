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
      idempotencyKey?: string|null;
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
      }| null;
      responseMappingOutput?: Record<string, unknown>;
    };
  }
  
  export interface ApiDefinition {
    id: string;
    version: string;
    routes: ApiRoute[];
    draft: boolean;
    createdAt?: string;
  }

  export interface OplogQueryParams {
    from?: number;
    count: number;
    cursor?: string;
    query?: string;
  }

  export type DeploymentApiDefinition = {
    id: string;
    version: string;
  }
  
  type Site = {
    host: string;
    subdomain: string;
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
  createdAt?: string;
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

export interface Plugin {
  name: string;
  version: string;
  description: string;
  icon: number[];
  homepage: string;
  specs: {
    type: "ComponentTransformer";
    providedWitPackage: string;
    jsonSchema: string;
    validateUrl: string;
    transformUrl: string;
  };
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
