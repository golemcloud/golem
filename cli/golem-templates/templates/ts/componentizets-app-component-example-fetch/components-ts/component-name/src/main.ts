/// <reference path="./generated/wit-generated.d.ts" />
import type * as bindings from "pack:name/component-name"

let result: any;

export const componentNameApi: typeof bindings.componentNameApi = {
    async getLastResult(): Promise<string> {
        return JSON.stringify(result);
    },
    async fetchJson(url: string): Promise<string> {
        const response = await fetch(url);
        const responseBody = await response.json();
        console.log(responseBody);
        return JSON.stringify(responseBody);
    },
}
