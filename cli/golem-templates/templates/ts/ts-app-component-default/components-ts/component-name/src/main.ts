import {
    BaseAgent,
    Agent,
    Prompt,
    Description,
} from '@golemcloud/golem-ts-sdk';

@Agent()
class AssistantAgent extends BaseAgent {
    @Prompt("Ask your question")
    @Description("This method allows the agent to answer your question")
    async ask(name: string): Promise<string> {
        const customData = { data: "Sample data", value: 42 };

        const remoteWeatherClient = WeatherAgent.createRemote("");
        const remoteWeather = await remoteWeatherClient.getWeather(name, customData);

        const localWeatherClient = WeatherAgent.createLocal("afsal");
        const localWeather = await localWeatherClient.getWeather(name, customData);

        return (
            `Hello! I'm the assistant agent (${this.getId()}) reporting on the weather in ${name}. ` +
            `Here’s what the weather agent says: "\n${localWeather}\n". ` +
            `Info retrieved using weather agent (${localWeatherClient.getId()}).`
        );
    }
}

@Agent()
class WeatherAgent extends BaseAgent {
    private readonly userName: string;

    constructor(username: string) {
        super()
        this.userName = username;
    }

    @Prompt("Get weather")
    @Description("Weather forecast weather for you")
    async getWeather(name: string, param2: CustomData): Promise<string> {
        return Promise.resolve(
            `Hi ${this.userName} Weather in ${name} is sunny. Params passed: ${name} ${JSON.stringify(param2)}. ` +
            `Computed by weather-agent ${this.getId()}. `
        );
    }
}

interface CustomData {
    data: String;
    value: number;
}
