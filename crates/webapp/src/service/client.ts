// import { ENDPOINT } from "./endpoints";
import {GolemService} from "@/service/base.ts";
import {Component} from "@/types/component.ts";
import {Api} from "@/types/api.ts";
import axios from "axios";
import {ENDPOINT} from "@/service/endpoints.ts";

const api = axios.create({
  baseURL: "http://localhost:9881"
})


export const APIService: GolemService = {
  createComponent(): Promise<any> {
    return Promise.resolve(undefined);
  },
  getComponentById(): Promise<any> {
    return Promise.resolve(undefined);
  },
  getComponents: async (): Promise<Component[]> => {
    return api.get(ENDPOINT.getComponents()).then((r) => JSON.parse(r.data) as Component[]);
  },
  getApiList: async (): Promise<Api[]> => {
    return api.get(ENDPOINT.getApiList()).then((r) => JSON.parse(r.data) as Api[]);
  },
  createApi: async (payload: Api) => {
    return api.post(ENDPOINT.createApi(), payload).then((r) => JSON.parse(r.data)); 
},
  getApi: async (id: string): Promise<Api[]> => {
    return api.get(ENDPOINT.getApi(id)).then((r) => JSON.parse(r.data) as Api[]);   
  },
  deleteApi: async (id: string, version: string) => { 
    return api.delete(ENDPOINT.deleteApi(id, version));     
  },
  putApi: async (id: string, version: string, payload: Api) => {
    return api.put(ENDPOINT.putApi(id, version), payload);
  },
}