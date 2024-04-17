let state = 0;

export const api = {
    setTimeout(time) {
        const start = Date.now();
        setTimeout(() => {
            state = Date.now();
        }, Number(time));

        return start;
    },

    get() {
        return state;
    }
}
