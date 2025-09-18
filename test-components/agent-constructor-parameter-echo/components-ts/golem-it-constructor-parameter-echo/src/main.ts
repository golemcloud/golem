import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';

@agent()
class EchoAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super()
        this.name = name;
    }

    async echo(): Promise<string> {
        return this.name
    }
}
