export interface Plugin {
  name: string;
  version: string;
  description: string;
  homepage: string;
  scope: string;
  type: string;
  oplogProcessorComponentId?: string;
  oplogProcessorComponentVersion?: number;
  // Legacy fields for backward compatibility if needed
  icon?: File[];
  specs?: {
    type: string;
    componentId?: string;
    componentVersion?: number;
    jsonSchema?: string;
    validateUrl?: string;
    transformUrl?: string;
  };
}

export interface PluginList {
  name: string;
  versions: Plugin[];
}
