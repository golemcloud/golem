import {
    BaseAgent,
    agent,
    endpoint,
    Principal
} from '@golemcloud/golem-ts-sdk';

@agent({
    mount: '/http-agents/{agentName}',
})
class HttpAgent extends BaseAgent {
    constructor(readonly agentName: string) {
        super();
    }

    // only Principal
    @endpoint({ get: "/echo-principal" })
    echoPrincipal(principal: Principal): { value: Principal } {
        return { value: principal }
    }

    // Principal in between
    @endpoint({ get: "/echo-principal-mid/{foo}/{bar}" })
    echoPrincipal2(foo: string, principal: Principal, bar: number): {value: Principal, foo: string, bar: number} {
        return {value: principal,  foo: foo, bar: bar};
    }

    // Principal at the end
    @endpoint({ get: "/echo-principal-last/{foo}/{bar}" })
    echoPrincipal3(foo: string, bar: number, principal: Principal): {value: Principal, foo: string, bar: number} {
        return {value: principal, foo, bar};
    }
}