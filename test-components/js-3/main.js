function asyncToSync(promise) {
    let success = false;
    let done = false;
    let result;
    let error;
    promise
        .then((r) => {
            result = r;
            success = true;
            done = true;
        })
        .catch((e) => {
            error = e;
            done = true;
        });
    runEventLoopUntilInterest();
    if (!done) {
        throw new Error("asyncToSync: illegal state: not done");
    }
    if (!success) {
        throw error;
    }
    return result;
}

export const api = {
    fetchGet(url) {
        console.log(`Calling fetch GET ${url}`);
        const result = asyncToSync(fetch(url).then((result) => result.text()));
        console.log(`Result:\n ${result}`);
        return result;
    },
}
