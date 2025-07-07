/// <reference path="./generated/wit-generated.d.ts" />
import type * as bindings from "pack:name/component-name"

// Use this import for using the common lib:
// import {example_common_function} from "common/lib";

let state = BigInt(0);

export const componentNameApi: typeof bindings.componentNameApi = {
    async add(value: bigint) {
        // Example common lib use:
        // console.log(example_common_function());
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    async get() {
        return state;
    }
};
