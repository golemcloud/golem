// import { ENDPOINT } from "./endpoints";
import {GolemService} from "@/service/base.ts";
import {Component} from "@/types/component.ts";
import axios from "axios";
import {invoke} from "@tauri-apps/api/core";
import {Response} from "@/types";
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
  // getComponents: Get the list of all components
  // https://release.api.golem.cloud/v1/components?project-id=305e832c-f7c1-4da6-babc-cb2422e0f5aa
  /**
   * Get the list of all components
   * @returns {Promise<Component[]>}
   */
  getComponents: async (): Promise<Component[]> => {
    return api.get(ENDPOINT.getComponents()).then((r) => JSON.parse(r.data) as Component[]);
    // return invoke("get_components").then((result) => {
    //   let res: Response<Component[]> = <Response<Component[]>> result;
    //   if(res.data == undefined){
    //     return [];
    //   }
    //   return res.data;
    // });
  }
}

