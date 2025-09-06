# Golem TypeScript SDK

```ts
import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';

type Input = {
    username: string,
    location: Location
}

type GeoLocation = { lat: number, long: number };

type Name = string;

type Place = Name | GeoLocation;

@agent()
class AssistantAgent extends BaseAgent {
    @prompt("Public weather forecast")
    @description("This method allows the agent to answer your question")
    async ask(input: Input): Promise<string> {
        const remoteWeatherClient = WeatherAgent.createRemote(input.username);
        return await remoteWeatherClient.getWeather(input.location);
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
    @description("Internal weather forecasting service. Only accessible if authorized.")
    async getWeather(location: Place): Promise<String> {
        return Promise.resolve(
            `Hi ${this.userName} ! Weather in ${location} is sunny. ` + 
            `Reported by weather-agent ${this.getId()}. `
        );
    }
}


```