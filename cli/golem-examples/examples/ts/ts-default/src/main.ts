import { Api } from './binding/component-name.js';

let state = BigInt(0);

export const api: Api = {
    add(value: bigint) {
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    get() {
        return state;
    }
}
