import { golemCreatePromise } from "golem:api/host@0.2.0";

var state = 0;

export const api = {
    add(value) {
        state += Number(value);
    },
    get() {
        console.log(`Returning the current counter value: ${state}`);
        return BigInt(state);
    },
    createPromise() {
        let promiseId = golemCreatePromise();
        console.log(`Created Promise`, promiseId);
        return promiseId
    }
}
