export const ENDPOINT = {
    getComponents: () => {
        return "/v1/components";
    },
    createComponent: () => {
        return "/v1/components";
    },
    deleteWorker: (id: string, workName: string) => {
        return `/v1/components/${id}/workers/${workName}`;
    },
    findWorker: (componentId: string) => {
        return `/v1/components/${componentId}/workers/find`;
    },
    getComponent: (id: string) => {
        return `/v1/components/${id}/latest`;
    },
    updateComponent: (id: string) => {
        return `/v1/components/${id}`;
    },
    getComponentById: (id: string) => {
        return `/v1/components/${id}/latest`;
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
    }
};
