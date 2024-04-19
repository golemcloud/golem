import { GolemItApi } from './interfaces/golem-it-api.js';

let state = 0;


// This uses BigInt because of overflow behavior.
export const api: typeof GolemItApi = {
    setTimeout(time) {
        const start = Date.now();
        setTimeout(() => {
            state = Date.now();
        }, Number(time));

        return BigInt(start);
    },

    get() {
        return BigInt(state);
    }
}
