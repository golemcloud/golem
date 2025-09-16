import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';


@agent()
class AssistantAgent extends BaseAgent {
    @prompt("Ask your question")
    @description("This method allows the agent to answer your question")
    async ask(_query: string): Promise<string> {
        const remoteWeatherClient = WeatherAgent.get("Jon");

        const location: Location = { place: { city: 'sydney' }, country: 'australia' };

        const remoteWeather =
            await remoteWeatherClient.getWeather(location);

        return (
            `Agent ${this.getId().value} reporting on the weather: ` +
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
    async getWeather(location: Location): Promise<string> {
        return Promise.resolve(
            `Weather in location ${JSON.stringify(location.place)} is sunny.`
        );
    }
}

type CityName = {
    city: string;
}

type PostalCode = {
    postalCode: number;
}

type Location = {
    place: PostalCode | CityName
    country: string;
};