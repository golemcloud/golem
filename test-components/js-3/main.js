export const api = {
    async fetchGet(url) {
        console.log(`Calling fetch GET ${url}`);
        const result = await fetch(url)
        const resultText = await result.text();
        console.log(`Result:\n ${resultText}`);
        return resultText;
    },
}
