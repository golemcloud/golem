
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
  }
};
