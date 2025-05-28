import { now } from "wasi:clocks/wall-clock@0.2.3";

export function hello(name) {
    console.log(`Hello ${name}!`);

    const random = Math.random();
    const randomString = crypto.randomUUID();

    const jsNow = Date.now();

    const { seconds, nanoseconds } = now();
    const wasiNow = Number(seconds) * 1000 + Math.floor(nanoseconds / 1000000);

    return {
        random,
        randomString,
        jsNow,
        wasiNow,
    };
}
