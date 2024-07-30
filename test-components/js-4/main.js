import { createPromise } from "golem:api/host@0.2.0";

export const api = {
    createPromise() {
        let promiseId = createPromise();
        console.log('Created Promise', promiseId);
        return promiseId
    }
}
