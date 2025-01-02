import {GolemService} from "@/service/base.ts";
import {Component} from "@/types/component.ts";
import {ENDPOINT} from "@/service/endpoints.ts";
import axios from "axios";
import {Api} from "@/types/api.ts";
import {fetch} from '@tauri-apps/plugin-http'


const api = axios.create({
    baseURL: "http://localhost:9881"
})


async function callApi(url: string, method: string = "GET", data: any = null): Promise<any> {
    // const r = await invoke("invoke_api", {url, method, data});
    console.log("callApi", url, method, data)
    const response = await fetch(`http://localhost:9881/${url}`, {
        method: method,
        body: data
    }).then(res => res.json())
    console.log("callApi", response)
    return response;
}


export async function callFormDataApi(formData: FormData) {


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
        return callApi(ENDPOINT.getComponents()).then((r) => r as Component[]);
        // return api.get(ENDPOINT.getComponents()).then((r) => JSON.parse(r.data) as Component[]);
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
    postApi: async (payload: Api) => {
        return api.post(ENDPOINT.postApi(), payload).then((r) => JSON.parse(r.data));
    },
    deleteApi: async (id: string, version: string) => {
        return api.delete(ENDPOINT.deleteApi(id, version));
    },
    putApi: async (id: string, version: string, payload: Api) => {
        return api.put(ENDPOINT.putApi(id, version), payload);
    },
    getWorkers: async (): Promise<{ cursor: string; workers: Worker[] }> => {
        return api.get(ENDPOINT.getWorkers()).then((r) => JSON.parse(r.data) as { cursor: string; workers: Worker[] });
    },
}

/// singleton service go call all the api
export class Service {
    private baseUrl: string;

    constructor(baseUrl: string = "http://localhost:9881") {
        this.baseUrl = baseUrl;
    }

    /**
     * getComponents: Get the list of all components
     * Note: Sample Endpoint https://release.api.golem.cloud/v1/components
     * @returns {Promise<Component[]>}
     */
    public getComponents = async (): Promise<Component[]> => {
        const r = await this.callApi(ENDPOINT.getComponents());
        return r as Component[];
    }

    public createComponent = async (form: FormData) => {
        // const headers = null
        const response = await fetch('http://localhost:9881/v1/components', {
            method: 'POST',
            body: form
        }).then(res => res.json())
        // const r = await this.callApi(ENDPOINT.createComponent(), 'POST', form, headers)
        //     .then(console.log).catch(console.log);
        return response;
    }

    public findWorker = async (componentId: string, param = {"count": 100, "precise": true}) => {
        const r = await this.callApi(ENDPOINT.findWorker(componentId), 'POST', JSON.stringify(param));
        return r;
    }

    public deleteWorker = async (componentId: string, workName: string) => {
        const r = await this.callApi(ENDPOINT.deleteWorker(componentId, workName), 'DELETE');
        return r;
    }

    public getComponentById = async (componentId: string) => {
        const r = await this.callApi(ENDPOINT.getComponentById(componentId));
        return r as Component;
    }

    public createWorker = async (componentID: string, params: any) => {
        const r = await this.callApi(ENDPOINT.createWorker(componentID), 'POST', JSON.stringify(params));
        return r;
    }

    public getComponentByIdAsKey = async (): Promise<Record<string, Component>> => {
        const result: Record<string, Component> = {};
        const components = await this.getComponents();
        components.forEach((data: Component) => {
            if (data?.versionedComponentId?.componentId) {
                // TODO: Need to check version is Latest or not
                result[data.versionedComponentId.componentId] = {
                    componentName: data.componentName,
                    componentId: data.versionedComponentId.componentId,
                    createdAt: data.createdAt,
                    exports: data?.metadata?.exports,
                    componentSize: data.componentSize,
                    componentType: data.componentType,
                    versionId: [
                        ...(result[data.versionedComponentId.componentId]
                            ?.versionId || []),
                        data.versionedComponentId.version,
                    ],
                };
            }
        });
        return result;
    }


    private callApi =
        async (url: string, method: string = "GET", data: FormData | string | null = null,
               headers = {'Content-Type': 'application/json'}): Promise<any> => {
            const resp = await fetch(`${this.baseUrl}${url}`, {
                method: method,
                body: data,
                headers: headers
            }).then(res => {
                if (res.ok) {
                    return res.json()
                } else {
                    res.json().then(console.log)
                    // console.log("callApi",)
                    throw res
                }

            }).catch(err => {
                console.log("callApi", err)
                throw err
            })
            console.log("callApi", resp)
            return resp;
        }

}
