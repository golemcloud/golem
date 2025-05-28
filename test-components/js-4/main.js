import { createPromise } from "golem:api/host@1.1.7";

export const api = {
    createPromise() {
        let promiseId = createPromise();
        console.log('Created Promise', promiseId);
        return promiseId
    }
}
