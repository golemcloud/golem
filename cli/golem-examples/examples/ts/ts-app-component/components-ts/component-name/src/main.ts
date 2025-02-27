import {ComponentNameApi} from "./generated/component-name";
// Use this import for using the common lib:
// import {example_common_function} from "common/lib";


let state = BigInt(0);

export const componentNameApi: ComponentNameApi = {
    add(value: bigint) {
        // Example common lib use:
        // console.log(example_common_function());
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    get() {
        return state;
    }
};
