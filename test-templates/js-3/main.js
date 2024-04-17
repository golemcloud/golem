let state = 0;
let timeout = null;

export const api = {
    setTimeout(value, time) {
        if (timeout === null) {
            const start = new Date();
            console.log(`Setting ${time}ms timeout. Value: ${value} at ${start}`);
            timeout = setTimeout(() => {
                let end = new Date();
                console.log(`Timeout triggered. Value: ${value} at ${end} (after ${end.getTime() - start.getTime()}ms)`);
                state = value;
                timeout = null;
            }, Number(time));
            return true;
        } else {
            console.log("Timeout already set");
            return false;
        }
    },

    clearTimeout() {
        state = 0;
        if (timeout !== null) {
            console.log("Clearing timeout");
            clearTimeout(timeout);
            timeout = null;
            return true;
        } else {
            return false;
        }
    },
    
    get() {
        return BigInt(state);
    }
}
