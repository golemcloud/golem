import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';
import {fork, type ForkResult} from "golem:api/host@1.1.7"

type CustomData = {
    data1?: string | null;
    data2?: string,
    data3: string | null,
    data4: string | number | null,
    data5?: string | number | null,
    data6?: string | number,
    data7?: string | undefined;
    data8?: string,
    data9: string | undefined,
    data10: string | number | undefined,
    data11?: string | number | undefined,
    data12?: string | number,
    data13: string | void,
    data14: string | number | void,
    data15?: string | number | void,
    value: number;
}

@agent()
class AssistantAgent extends BaseAgent {
    @prompt("Ask your question")
    @description("This method allows the agent to answer your question")
    async ask(name: string | number | null): Promise<string> {
        const customData: CustomData = {
            data1: 'Hello World',
            data2: 'Hello World!',
            data3: 'Hello World!',
            data4: 'Hello World!',
            data5: 'Hello World!',
            data6: 'Hello World!',
            data7: 'Hello World!',
            data8: 'Hello World!',
            data9: 'Hello World!',
            data10: 'Hello World!',
            data13: undefined,
            data14: 1,
            data15: undefined,
            value: 42
        };

        const remoteWeatherClient = WeatherAgent.get("Jon");
        const remoteWeather = await remoteWeatherClient.getWeather("abc", customData);

        return (
            `${this.getId().value}) reporting on the weather in ${name}: ` +
            remoteWeather
        );
    }
}

@agent()
class WeatherAgent extends BaseAgent {
    private readonly userName: string | null;

    constructor(username: string | null) {
        super()
        this.userName = username;
    }

    @prompt("Get weather")
    @description("Weather forecast weather for you")
    async getWeather(name: string | undefined, param2: CustomData): Promise<string> {
        return Promise.resolve(
            `Weather in ${name} is sunny.`
        );
    }
}

