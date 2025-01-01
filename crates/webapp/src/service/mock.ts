import {GolemService} from "@/service/base";
import {Component} from "@/types/component";
import {Api} from "@/types/api";

// @ts-ignore
export const MockService: GolemService = {
  createComponent: async () => {
    return Promise.resolve(undefined);
  },
  getComponentById: async () => {
    return Promise.resolve(undefined);
  },
  getComponents: async (): Promise<Component[]> => {
      return import("@/mocks/get_components.json").then((res) => res.default as Component[]);
  },
  getApiList: async (): Promise<Api[]> => {
      return import("@/mocks/get_apiList.json").then((res) => res.default as Api[]);
  },
  createApi: async (payload: Api) => {
    console.log(payload);
    return Promise.resolve(undefined);
  },
  getApi: async (id: string): Promise<Api[]> => {
      console.log(id);
      return import("@/mocks/get_api.json").then((res) => res.default as Api[]);
  },
  deleteApi: async (id: string, version: string) => {
    console.log(id, version);
    return Promise.resolve(undefined);
  },
  putApi: async (id: string, version: string, payload: Api) => {
    console.log(id, version, payload);
    return Promise.resolve(undefined);
  },
};