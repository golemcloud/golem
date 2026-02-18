import {
    BaseAgent,
    agent,
    prompt,
    description,
    endpoint
} from '@golemcloud/golem-ts-sdk';

@agent({
  mount: "/counters/{name}"
})
class CounterAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super()
        this.name = name;
    }

    @prompt("Increase the count by one")
    @description("Increases the count by one and returns the new value")
    @endpoint({ post: "/increment" })
    async increment(): Promise<number> {
        this.value += 1;
        return this.value;
    }

    override async saveSnapshot(): Promise<Uint8Array> {
        const snapshot = new Uint8Array(4);
        const view = new DataView(snapshot.buffer);
        view.setUint32(0, this.value);
        console.info(`Saved snapshot: ${this.value}`);
        return snapshot;
    }

    override async loadSnapshot(bytes: Uint8Array): Promise<void> {
        let view = new DataView(bytes.buffer);
        this.value = view.getUint32(0);
        console.info(`Loaded snapshot!: ${this.value}`);
    }
}
