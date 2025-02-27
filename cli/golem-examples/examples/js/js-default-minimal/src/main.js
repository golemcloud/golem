let state = BigInt(0);

export const api = {
    add(value) {
        console.log(`Adding ${value} to the counter`);
        state += value;
    },
    get() {
        return state;
    }
}
