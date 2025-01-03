import {Component} from "@/types/component.ts";
import {Api} from "@/types/api.ts";
import {Worker} from "@/types/worker.ts";


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
    getParticularWorker(componentId: string, workerName: string): Promise<Worker>;
    interruptWorker(componentId: string, workerName: string): Promise<any>;
    resumeWorker(componentId: string, workerName: string): Promise<any>;
    deleteWorker(componentId: string, workName: string): Promise<any>;
    invokeWorkerAwait(componentId: string, workerName: string, functionName: string, payload: any): Promise<any>; 
}