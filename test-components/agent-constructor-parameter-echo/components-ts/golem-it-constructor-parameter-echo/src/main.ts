import {
    BaseAgent,
    agent
} from '@golemcloud/golem-ts-sdk';

@agent()
class EchoAgent extends BaseAgent {
    private name: string;

    constructor(name: string) {
        super()
        this.name = name;
    }

    async echo(): Promise<string> {
        return this.name
    }

    /// A method that appends a '!' to the returned string every time it's called.
    async changeAndGet(): Promise<string> {
        this.name = this.name + "!";
        return this.name;
    }
}

@agent({ mode: 'ephemeral' })
class EphemeralEchoAgent extends BaseAgent {
  private name: string;

  constructor(name: string) {
      super()
      this.name = name;
  }

  async echo(): Promise<string> {
      return this.name
  }

  /// A method that appends a '!' to the returned string every time it's called.
  async changeAndGet(): Promise<string> {
      this.name = this.name + "!";
      return this.name;
  }
}
