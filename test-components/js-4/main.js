import { golemCreatePromise } from "golem:api/host@0.2.0";

export const api = {
    createPromise() {
        let promiseId = golemCreatePromise();
        console.log('Created Promise', promiseId);
        return promiseId
    }
}
