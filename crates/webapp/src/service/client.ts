/* eslint-disable @typescript-eslint/no-explicit-any */
import {Component} from "@/types/component.ts";
import {ENDPOINT} from "@/service/endpoints.ts";
import {Api} from "@/types/api.ts";
import {fetch} from '@tauri-apps/plugin-http'
import {toast} from "@/hooks/use-toast"
import {Plugin} from "@/types";


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

    public updateComponent = async (componenetId: string, form: FormData) => {
        // const headers = null
        const response = await fetch(`http://localhost:9881/v1/components/${componenetId}/updates`, {
            method: 'POST',
            body: form
        }).then(res => res.json())
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

    public getApiList = async (): Promise<Api[]> => {
        const r = await this.callApi(ENDPOINT.getApiList());
        return r
    }

    public getApi = async (id: string): Promise<Api[]> => {
        const r = await this.callApi(ENDPOINT.getApi(id));
        return r
    }

    public createApi = async (payload: Api) => {
        return await this.callApi(ENDPOINT.createApi(), "POST", JSON.stringify(payload));
    }

    public deleteApi = async (id: string, version: string) => {
        return await this.callApi(ENDPOINT.deleteApi(id, version), "DELETE");
    }

    public putApi = async (id: string, version: string, payload: Api) => {
        const r = await this.callApi(ENDPOINT.putApi(id, version), "PUT", JSON.stringify(payload));
        return r
    }

    public postApi = async (payload: Api) => {
        const r = await this.callApi(ENDPOINT.postApi(), "POST", JSON.stringify(payload));
        return r
    }

    public getParticularWorker = async (componentId: string, workerName: string) => {
        const r = await this.callApi(ENDPOINT.getParticularWorker(componentId, workerName));
        return r
    }

    public interruptWorker = async (componentId: string, workerName: string) => {
        const r = await this.callApi(ENDPOINT.interruptWorker(componentId, workerName), "POST", JSON.stringify({}));
        return r
    }

    public resumeWorker = async (componentId: string, workerName: string) => {
        const r = await this.callApi(ENDPOINT.resumeWorker(componentId, workerName), "POST", JSON.stringify({}));
        return r
    }

    public invokeWorkerAwait = async (componentId: string, workerName: string, functionName: string, payload: any) => {
        const r = await this.callApi(ENDPOINT.invokeWorker(componentId, workerName, functionName), "POST", JSON.stringify(payload));
        return r
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

    public getPlugins = async (): Promise<Plugin[]> => {
        return await this.callApi(ENDPOINT.getPlugins());
    }

    public getPluginByName = async (name: string): Promise<Plugin[]> => {
        return await this.callApi(ENDPOINT.getPluginName(name));
    }

    public downloadComponent = async (componentId: string, version: string): Promise<any> => {
        return await this.downloadApi(ENDPOINT.downloadComponent(componentId, version));
    }
    public createPlugin = async (payload: FormData) => {
        return await this.callApi(ENDPOINT.getPlugins(), 'POST', JSON.stringify(payload));
    }
    public deletePlugin = async (name: string, version: string) => {
        return await this.callApi(ENDPOINT.deletePlugin(name, version), 'DELETE');
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
                    console.log(res)
                    return res.json()
                } else {
                    if(method !== "GET") {
                        let errorTitle = "Api is Failed check the api details"
                        console.log(res)
                        res.json().then(err => {
                            errorTitle = err?.golemError?.message || errorTitle
                        })
                        toast({
                            title: errorTitle,
                            variant: "destructive",
                        })
                    }
                    throw res;
                }

            }).catch(err => {
                if(method !== "GET") {
                toast({
                    title: "Api is Failed check the api details",
                    variant: "destructive",
                });
                }
                throw err
            })
            console.log("callApi", resp)
            return resp;
        }


        private downloadApi =
        async (url: string, method: string = "GET", data: FormData | string | null = null,
               headers = {'Content-Type': 'application/json'}): Promise<any> => {
            const resp = await fetch(`${this.baseUrl}${url}`, {
                method: method,
                body: data,
                headers: headers
            }).then(res => {
                if (res.ok) {
                    console.log(res)
                    return res
                }
            }).catch(err => {
                toast({
                    title: "Api is Failed check the api details",
                    variant: "destructive",
                });
                throw err
            })
            console.log("callApi", resp)
            return resp;
        }

}
