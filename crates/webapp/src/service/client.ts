import { Component } from "@/types/component";
import { ENDPOINT } from "./endpoints";


export class ComponentClient {
  getComponents: () => Promise<Component[]> = async () => {
    const res = await fetch(ENDPOINT.getComponents());
    return await res.json();
  };
}

export const componentClient: ComponentClient = {
  getComponents: async () => {
    return [];
  },
};
