export const ENDPOINT = {
  getComponents: () => {
    return "/v1/components";
  },
  createComponent: () => {
    return "/v1/components";
  },
  getComponentByIdAndVersion: (id: string, version: number) => {
    return `/v1/components/${id}/versions/${version}`;
  },
  addPluginToComponent: (id: string) => {
    return `/v1/components/${id}/latest/plugins/installs`;
  },
  deletePluginToComponent: (id: string, installation_id: string) => {
    return `/v1/components/${id}/latest/plugins/installs/${installation_id}`;
  },
  deleteAgent: (id: string, workName: string) => {
    return `/v1/components/${id}/agents/${workName}`;
  },
  findAgent: (componentId: string) => {
    return `/v1/components/${componentId}/agents/find`;
  },
  updateComponent: (id: string) => {
    return `/v1/components/${id}`;
  },
  getComponentById: (id: string) => {
    return `/v1/components/${id}`;
  },
  getApiList: () => {
    return "/v1/api/definitions";
  },
  createApi: () => {
    return "/v1/api/definitions";
  },
  postApi: () => {
    return `/v1/api/definitions`;
  },
  getApi: (id: string) => {
    return `/v1/api/definitions?api-definition-id=${id}`;
  },
  deleteApi: (id: string, version: string) => {
    return `/v1/api/definitions/${id}/${version}`;
  },
  putApi: (id: string, version: string) => {
    return `/v1/api/definitions/${id}/${version}`;
  },
  getAgents: () => {
    return "/v1/components/agents/find";
  },
  createAgent: (component_id: string) => {
    return `/v1/components/${component_id}/agents`;
  },
  getParticularAgent: (componentId: string, agentName: string) => {
    return `/v1/components/${componentId}/agents/${agentName}`;
  },
  interruptAgent: (componentId: string, agentName: string) => {
    return `/v1/components/${componentId}/agents/${agentName}/interrupt`;
  },
  resumeAgent: (componentId: string, agentName: string) => {
    return `/v1/components/${componentId}/agents/${agentName}/resume`;
  },
  updateAgent: (componentId: string, agentName: string) => {
    return `/v1/components/${componentId}/agents/${agentName}/update`;
  },
  invokeAgent: (
    componentId: string,
    agentName: string,
    functionName: string,
  ) => {
    return `/v1/components/${componentId}/agents/${agentName}/invoke-and-await?function=${functionName}`;
  },
  invokeEphemeralAgent: (componentId: string, functionName: string) => {
    return `/v1/components/${componentId}/invoke-and-await?function=${functionName}`;
  },
  getPlugins: () => {
    return "/v1/plugins";
  },
  getPluginName: (name: string) => {
    return `/v1/plugins/${name}`;
  },
  downloadComponent: (componentId: string, version: number) => {
    return `/v1/components/${componentId}/download?version=${version}`;
  },
  deletePlugin: (name: string, version: string) => {
    return `/v1/plugins/${name}/${version}`;
  },
  getDeploymentApi: (versionId: string) => {
    return `/v1/api/deployments?api-definition-id=${versionId}`;
  },
  deleteDeployment: (deploymentId: string) => {
    return `/v1/api/deployments/${deploymentId}`;
  },
  createDeployment: () => {
    return `/v1/api/deployments/deploy`;
  },
  getOplog: (
    componentId: string,
    agentName: string,
    count: number,
    searchQuery: string,
  ) => {
    return `/v1/components/${componentId}/agents/${agentName}/oplog?count=${count}&query=${searchQuery}`;
  },
};
