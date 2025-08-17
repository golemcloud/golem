import type * as bindings from "component-name"

let result: any = undefined;

export const componentNameApi: typeof bindings.componentNameApi = {
    async getLastResult() {
        return result ? JSON.stringify(result) : "???";
    },
    async fetchJson(url) {
        const response = await fetch(url);
        const responseBody = await response.json();
        console.log(responseBody);

        result = responseBody; // Store the result for later retrieval

        return JSON.stringify(responseBody);
    },
}
