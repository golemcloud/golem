var state = 0;

export const api = {
    add(value) {
        console.log(`Adding ${value} to the counter`);
        state += Number(value);
    },
    get() {
        console.log(`Returning the current counter value: ${state}`);
        return BigInt(state);
    }
}
