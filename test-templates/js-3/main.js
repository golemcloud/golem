var state = 0;

export const api = {
    async fetchPost() {
        console.log("Fetching post");
        let response = await fetch("https://jsonplaceholder.typicode.com/posts/1");
        // let json = await response.json();
        // console.log("Retrieved Post 1", json);
        state += 1;
        return response.status
    },
    get() {
        console.log(`Returning the current counter value: ${state}`);
        return BigInt(state);
    }
}
