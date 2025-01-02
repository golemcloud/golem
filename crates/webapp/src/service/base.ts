import {Component} from "@/types/component.ts";
import {Api} from "@/types/api.ts";


export interface GolemService {
    getComponents(): Promise<Component[]>;
    getApiList(): Promise<Api[]>;
    postApi(payload: Api): Promise<Api>;
    getApi(id: string): Promise<Api[]>;
    deleteApi(id: string, version: string): Promise<any>;
    putApi(id: string, version: string, payload: Api): Promise<any>;
    createApi(payload: Api): Promise<any>;
    createComponent(): Promise<any>;
    getComponentById(): Promise<any>;
    getWorkers(): Promise<any>;
}