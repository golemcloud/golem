import type * as bindings from "component-name"

// Use this import for using the common lib:
// import {exampleCommonFunction} from "common/lib";

let state = 0;

export const componentNameApi: typeof bindings.componentNameApi = {
    async add(value: number) {
        // Example common lib use:
        // console.log(example_common_function());
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    async get() {
        return state;
    }
};
