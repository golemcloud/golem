let result = undefined;

export const componentNameApi = {
    async getLastResult() {
        return result ? JSON.stringify(result) : undefined;
    },
    async fetchJson(url) {
        const response = await fetch(url);
        const responseBody = await response.json();
        console.log(responseBody);

        result = responseBody; // Store the result for later retrieval

        return JSON.stringify(responseBody);
    },
}
