import { now } from "wasi:clocks/wall-clock@0.2.0-rc-2023-11-10";

export function hello(name) {
    let x = Math.random();
    let dateNow = Date.now();
    let y = new Date(dateNow);
    let z = now();
    let wasiDate = new Date(Number(z.seconds) * 1000 + (z.nanoseconds / 1000000));

    const output = `Hello ${name}! ${x} ${dateNow} ${y} ${y.getFullYear()}} ${z.seconds} ${wasiDate} ${wasiDate.getFullYear()}`;
    console.log(output);
    return output;
}
