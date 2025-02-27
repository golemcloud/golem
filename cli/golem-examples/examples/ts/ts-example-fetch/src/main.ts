import {asyncToSyncAsResult} from "@golemcloud/golem-ts";
import { Api } from './generated/component-name';

let result: any;

export const api: Api = {
    getLastResult(): string {
        return JSON.stringify(result);
    },
    fetchJson(url: string): string {
        result = asyncToSyncAsResult(fetch(url).then(response => response.json()));
        console.log(result);
        return JSON.stringify(result);
    },
}
