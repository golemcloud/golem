import {GolemService} from "@/service/base";
import {Component} from "@/types/component";

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
};