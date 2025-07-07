let state = BigInt(0);

export const componentNameApi = {
    add(value) {
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    get() {
        return state;
    }
}
