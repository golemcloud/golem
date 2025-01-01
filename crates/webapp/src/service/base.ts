import {Component} from "@/types/component.ts";


export interface GolemService {
    getComponents(): Promise<Component[]>;
    createComponent(): Promise<any>;
    getComponentById(): Promise<any>;
}