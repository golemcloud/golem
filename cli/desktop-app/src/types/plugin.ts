export interface Plugin {
  name: string;
  version: string;
  description: string;
  homepage: string;
  scope: string;
  type: string;
  oplogProcessorComponentId?: string;
  oplogProcessorComponentVersion?: number;
  icon?: File[];
  specs?: {
    type: string;
    componentId?: string;
    componentVersion?: number;
  };
}

export interface PluginList {
  name: string;
  versions: Plugin[];
}
