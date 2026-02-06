export interface Plugin {
  accountId: string;
  id: string;
  name: string;
  version: string;
  description: string;
  homepage: string;
  // scope: string;
  type: string;
  oplogProcessorComponentId?: string;
  oplogProcessorComponentRevision?: number;
  // Legacy fields for backward compatibility if needed
  icon?: File[];
  spec?: {
    type: string;
    componentId?: string;
    componentRevision?: number;
    jsonSchema?: string;
    validateUrl?: string;
    transformUrl?: string;
  };

  // accountId: "51de7d7d-f286-49aa-b79a-96022f7e2df9"

  // description: "described component"

  // homepage: "https://example.com"

  // icon: "/9j/4AAQSkZJRgABAQAAAQABAAD/2wCEAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSgBBwcHCggKEwoKEygaFhoo…"

  // id: "019c0feb-3923-75d1-b982-ad5302d69907"

  // name: "my-plugin"

  // spec: {type: "App"}

  // version: "0.0.1"
}

export interface PluginList {
  name: string;
  versions: Plugin[];
}
