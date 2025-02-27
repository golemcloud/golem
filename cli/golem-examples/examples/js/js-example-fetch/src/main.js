import {asyncToSyncAsResult} from "@golemcloud/golem-ts";

let result;

export const api = {
    getLastResult() {
        return JSON.stringify(result);
    },
    fetchJson(url) {
        result = asyncToSyncAsResult(fetch(url).then(response => response.json()));
        console.log(result);
        return JSON.stringify(result);
    },
}
