export interface Typ {
  type: string;
  fields?: Field[];
  cases?: Case[] | string[];
  inner?: Typ;
  ok?: Typ;
  err?: Typ;
  names?: string[];
}

export interface Field {
  name: string;
  typ: Typ;
}

export type TypeField = {
  name: string;
  typ: {
    type: string;
    inner?: Field["typ"];
    fields?: Field[];
    cases?: Array<string | { name: string; typ: Field["typ"] }>;
    names?: string[];
    ok?: Field["typ"];
    err?: Field["typ"];
  };
};

export interface Case {
  name: string;
  typ: Typ;
}

export interface Function {
  name: string;
  parameters: Parameter[];
  results: Result[];
}

export interface Parameter {
  type: string;
  name: string;
  typ: Typ;
}

export interface Result {
  name: string | null;
  typ: Typ;
}

export interface Export {
  name: string;
  type: string;
  functions: Function[];
}

export interface Memory {
  initial: number;
  maximum: number | null;
}

export interface Value {
  name: string;
  version: string;
}

export interface FieldProducer {
  name: string;
  values: Value[];
}

export interface Producer {
  fields: FieldProducer[];
}

export interface Metadata {
  exports: Export[];
  memories: Memory[];
  producers: Producer[];
}

export interface VersionedComponentId {
  componentId?: string;
  version?: number;
}

export enum ComponentType {
  Durable = "Durable",
  Ephemeral = "Ephemeral",
}

export interface Component {
  componentName?: string;
  componentSize?: number;
  componentType?: ComponentType;
  createdAt?: string;
  files?: FileStructure[];
  installedPlugins?: InstalledPlugin[];
  metadata?: Metadata;
  projectId?: string;
  componentId?: string;
  exports?: Export[];
  versionedComponentId?: VersionedComponentId;
}

export interface FileStructure {
  key: string;
  path: string;
  permissions: string;
}

export interface InstalledPlugin {
  id: string;
  name: string;
  version: string;
  priority: number;
  parameters: unknown;
}

export interface ComponentList {
  componentName?: string;
  componentType?: string;
  versions?: Component[];
  versionList?: number[];
  componentId?: string;
}

export interface ComponentExportFunction {
  name: string;
  parameters: Parameter[];
  results: Result[];
  exportName?: string;
}
