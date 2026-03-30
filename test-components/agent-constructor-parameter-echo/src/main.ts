import {
    BaseAgent,
    agent
} from '@golemcloud/golem-ts-sdk';
import { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';

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

@agent({ snapshotting: { every: 1 } })
class SqliteSnapshotAgent extends BaseAgent {
    private memDb: DatabaseSync;
    private fileDb: DatabaseSync;
    private label: string;

    constructor(id: string) {
        super();
        this.label = 'initial';
        this.memDb = new DatabaseSync(':memory:');
        this.memDb.exec('CREATE TABLE items (id INTEGER PRIMARY KEY AUTOINCREMENT, value TEXT)');
        try { mkdirSync('/tmp'); } catch (_) {}
        this.fileDb = new DatabaseSync('/tmp/sqlite-snapshot-test.db');
        this.fileDb.exec('CREATE TABLE log (id INTEGER PRIMARY KEY AUTOINCREMENT, message TEXT)');
    }

    async addItem(value: string): Promise<number> {
        const stmt = this.memDb.prepare('INSERT INTO items (value) VALUES (?)');
        stmt.run(value);
        const row = this.memDb.prepare('SELECT last_insert_rowid() as id').get() as { id: number };
        return row.id;
    }

    async addLog(message: string): Promise<number> {
        const stmt = this.fileDb.prepare('INSERT INTO log (message) VALUES (?)');
        stmt.run(message);
        const row = this.fileDb.prepare('SELECT last_insert_rowid() as id').get() as { id: number };
        return row.id;
    }

    async setLabel(label: string): Promise<void> {
        this.label = label;
    }

    async getState(): Promise<string> {
        const items = this.memDb.prepare('SELECT value FROM items ORDER BY id').all() as Array<{ value: string }>;
        const logs = this.fileDb.prepare('SELECT message FROM log ORDER BY id').all() as Array<{ message: string }>;
        return JSON.stringify({
            label: this.label,
            items: items.map(r => r.value),
            logs: logs.map(r => r.message),
        });
    }
}

