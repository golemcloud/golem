export interface Plugin {
  name: string;
  version: string;
  description: string;
  homepage: string;
  icon: File[];
  specs: {
    type: string;
    componentId?: string;
    componentVersion?: number;
    jsonSchema?: string;
    validateUrl?: string;
    transformUrl?: string;
  };
  scope: {
    type: string;
    componentID?: string;
  };
}

export interface PluginList {
  name: string;
  version: string[];
}
