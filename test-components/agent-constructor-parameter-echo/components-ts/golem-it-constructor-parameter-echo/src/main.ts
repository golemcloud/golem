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

    async returnInput(input: string): Promise<string> {
        return input;
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

@agent({ snapshotting: { every: 1 } })
class SnapshotCounterAgent extends BaseAgent {
    private count: number;

    constructor(id: string) {
        super();
        this.count = 0;
    }

    async increment(): Promise<number> {
        this.count += 1;
        return this.count;
    }

    async get(): Promise<number> {
        return this.count;
    }
}
