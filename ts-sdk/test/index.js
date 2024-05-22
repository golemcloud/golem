import { golemCreatePromise } from "golem-ts";

export const api = {
    createPromise() {
        let promiseId = golemCreatePromise();
        console.log('Created Promise', promiseId);
        return promiseId
    }
}
