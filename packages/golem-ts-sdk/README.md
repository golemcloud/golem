# Golem TypeScript SDK

```ts
import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';

type CustomData = {
    data: string;
    value: number;
}

@agent()
class AssistantAgent extends BaseAgent {
    @prompt("Ask your question")
    @description("This method allows the agent to answer your question")
    async ask(name: string): Promise<string> {
        const customData = { data: "Sample data", value: 42 };

        const remoteWeatherClient = WeatherAgent.get("Jon");
        const remoteWeather = await remoteWeatherClient.getWeather(name, customData);

        return (
            `${this.getId().value} reporting on the weather in ${name}: ` +
            remoteWeather
        );
    }
}

@agent()
class WeatherAgent extends BaseAgent {
    private readonly userName: string;

    constructor(username: string) {
        super()
        this.userName = username;
    }

    @prompt("Get weather")
    @description("Weather forecast weather for you")
    async getWeather(name: string, param2: CustomData): Promise<string> {
        return Promise.resolve(
            `Weather in ${name} is sunny.`
        );
    }
}


```
