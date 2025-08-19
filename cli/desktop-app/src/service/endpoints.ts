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
  deleteWorker: (id: string, workName: string) => {
    return `/v1/components/${id}/workers/${workName}`;
  },
  findWorker: (componentId: string) => {
    return `/v1/components/${componentId}/workers/find`;
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
  getWorkers: () => {
    return "/v1/components/workers/find";
  },
  createWorker: (component_id: string) => {
    return `/v1/components/${component_id}/workers`;
  },
  getParticularWorker: (componentId: string, workerName: string) => {
    return `/v1/components/${componentId}/workers/${workerName}`;
  },
  interruptWorker: (componentId: string, workerName: string) => {
    return `/v1/components/${componentId}/workers/${workerName}/interrupt`;
  },
  resumeWorker: (componentId: string, workerName: string) => {
    return `/v1/components/${componentId}/workers/${workerName}/resume`;
  },
  updateWorker: (componentId: string, workerName: string) => {
    return `/v1/components/${componentId}/workers/${workerName}/update`;
  },
  invokeWorker: (
    componentId: string,
    workerName: string,
    functionName: string,
  ) => {
    return `/v1/components/${componentId}/workers/${workerName}/invoke-and-await?function=${functionName}`;
  },
  invokeEphemeralWorker: (componentId: string, functionName: string) => {
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
    workerName: string,
    count: number,
    searchQuery: string,
  ) => {
    return `/v1/components/${componentId}/workers/${workerName}/oplog?count=${count}&query=${searchQuery}`;
  },
};
