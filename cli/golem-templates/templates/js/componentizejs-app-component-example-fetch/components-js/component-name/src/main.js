
export const componentNameApi = {
    async getLastResult() {
        return JSON.stringify(result);
    },
    async fetchJson(url) {
        const response = await fetch(url);
        const responseBody = response.json();
        console.log(responseBody);
        return JSON.stringify(responseBody);
    },
}
