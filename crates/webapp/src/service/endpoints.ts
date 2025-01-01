import { create } from "domain";

export const ENDPOINT = {
  getComponents: () => {
    return "/v1/components";
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
  getApi: (id: string) => {
    return `/v1/api/definitions?api-definition-id=${id}`;
  },
  deleteApi: (id: string, version: string) => {
    return `/v1/api/definitions/${id}/${version}`;  
  },
  putApi: (id: string, version: string) => {
    return `/v1/api/definitions/${id}/${version}`;
  }
};
