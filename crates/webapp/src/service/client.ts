// import { ENDPOINT } from "./endpoints";
import {GolemService} from "@/service/base.ts";
import {Component} from "@/types/component.ts";
// import axios from "axios";
import {ENDPOINT} from "@/service/endpoints.ts";
import {invoke} from "@tauri-apps/api/core";

// const api = axios.create({
//   baseURL: "http://localhost:9881"
// })

enum ApiRequestStatus {
  Success = "success",
  Error = "error"
}
interface ApiResponse {
  status: ApiRequestStatus;
  data?: any;
  error?: any;
}

async function callApi(url: string, method: string = "GET", data: any = null): Promise<ApiResponse | undefined> {
  const r = await invoke("invoke_api", {url, method, data});
  let result = r as ApiResponse;
  if (result.status === "error") {
    console.log(result.error);
    return;
  }
  return result;
}

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
    return callApi(ENDPOINT.getComponents()).then((r) => r?.data as Component[]);
  }
}

