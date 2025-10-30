import { agent, BaseAgent, } from '@golemcloud/golem-ts-sdk';

@agent()
class TestAgent extends BaseAgent {
  private readonly name: string;

  constructor(name: string) {
    super()
    this.name = name;
  }

  async testAll() {
    await this.testString();
    await this.testStruct();
  }

  async testString() {
    for (let i = 445; i < 450; i++) {
      await StringAgent.get(' '.repeat(i)).test();
    }
  }

  async testStruct() {
    for (let i = 100; i < 105; i++) {
      await StructAgent.get({
        x: ' '.repeat(i),
        y: ' '.repeat(i),
        z: '/'.repeat(i)
      }).test();
    }
  }
}

@agent()
class StringAgent extends BaseAgent {
  constructor(name: string) {
    super()
  }

  test() {
  }
}

@agent()
class StructAgent extends BaseAgent {
  constructor(args: { x: string, y: string, z: string }) {
    super()
  }

  test() {
  }
}
