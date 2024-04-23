import { golemCreatePromise } from "wasi:golem/golem:api@0.2.0";

var state = 0;

export const api = {
    add(value) {
        let promiseId = golemCreatePromise();
        console.log(`Adding ${value} to the counter`, promiseId);
        state += Number(value);
    },
    get() {
        console.log(`Returning the current counter value: ${state}`);
        return BigInt(state);
    }
}
