/* eslint-disable @typescript-eslint/no-explicit-any */
export interface Typ {
    type: string;
    fields?: Field[];
    cases?: Case[];
    inner?: Typ;
}

export interface Field {
    name: string;
    typ: Typ;
}

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
    files?: any[];
    installedPlugins?: any[];
    metadata?: Metadata;
    projectId?: string;
    versionId?: any[];
    componentId?: string;
    exports?: any[];
    versionedComponentId?: VersionedComponentId;
}

export interface ComponentExportFunction {
   name: string;
   parameters: Parameter[]; 
   results: Result[];
}

