import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';

export const EchoAgent = defineAgent({
    name: 'EchoAgent',
    id: { name: z.string() },
    methods: {
        echo: method({ input: {}, returns: z.string() }),
        returnInput: method({ input: { input: z.string() }, returns: z.string() }),
        changeAndGet: method({ input: {}, returns: z.string() }),
    },
});

export const EchoAgentImpl = EchoAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async echo() {
            return this.name;
        },
        async returnInput({ input }) {
            return input;
        },
        /// A method that appends a '!' to the returned string every time it's called.
        async changeAndGet() {
            this.name = this.name + '!';
            return this.name;
        },
    },
});

export const EphemeralEchoAgent = defineAgent({
    name: 'EphemeralEchoAgent',
    id: { name: z.string() },
    mode: 'ephemeral',
    methods: {
        echo: method({ input: {}, returns: z.string() }),
        changeAndGet: method({ input: {}, returns: z.string() }),
    },
});

export const EphemeralEchoAgentImpl = EphemeralEchoAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async echo() {
            return this.name;
        },
        /// A method that appends a '!' to the returned string every time it's called.
        async changeAndGet() {
            this.name = this.name + '!';
            return this.name;
        },
    },
});

export const SnapshotCounterAgent = defineAgent({
    name: 'SnapshotCounterAgent',
    id: { id: z.string() },
    snapshotting: { everyNInvocations: 1 },
    methods: {
        increment: method({ input: {}, returns: z.number() }),
        get: method({ input: {}, returns: z.number() }),
    },
});

export const SnapshotCounterAgentImpl = SnapshotCounterAgent.implement({
    init: () => ({ count: 0 }),
    methods: {
        async increment() {
            this.count += 1;
            return this.count;
        },
        async get() {
            return this.count;
        },
    },
});

export const SqliteSnapshotAgent = defineAgent({
    name: 'SqliteSnapshotAgent',
    id: { id: z.string() },
    snapshotting: { everyNInvocations: 1 },
    methods: {
        addItem: method({ input: { value: z.string() }, returns: z.number() }),
        addLog: method({ input: { message: z.string() }, returns: z.number() }),
        setLabel: method({ input: { label: z.string() }, returns: z.void() }),
        getState: method({ input: {}, returns: z.string() }),
    },
});

export const SqliteSnapshotAgentImpl = SqliteSnapshotAgent.implement({
    init: () => {
        const memDb = new DatabaseSync(':memory:');
        memDb.exec('CREATE TABLE items (id INTEGER PRIMARY KEY AUTOINCREMENT, value TEXT)');
        try { mkdirSync('/tmp'); } catch (_) {}
        const fileDb = new DatabaseSync('/tmp/sqlite-snapshot-test.db');
        fileDb.exec('CREATE TABLE log (id INTEGER PRIMARY KEY AUTOINCREMENT, message TEXT)');
        return { label: 'initial', memDb, fileDb };
    },
    methods: {
        async addItem({ value }) {
            const stmt = this.memDb.prepare('INSERT INTO items (value) VALUES (?)');
            stmt.run(value);
            const row = this.memDb.prepare('SELECT last_insert_rowid() as id').get() as { id: number };
            return row.id;
        },
        async addLog({ message }) {
            const stmt = this.fileDb.prepare('INSERT INTO log (message) VALUES (?)');
            stmt.run(message);
            const row = this.fileDb.prepare('SELECT last_insert_rowid() as id').get() as { id: number };
            return row.id;
        },
        async setLabel({ label }) {
            this.label = label;
        },
        async getState() {
            const items = this.memDb.prepare('SELECT value FROM items ORDER BY id').all() as Array<{ value: string }>;
            const logs = this.fileDb.prepare('SELECT message FROM log ORDER BY id').all() as Array<{ message: string }>;
            return JSON.stringify({
                label: this.label,
                items: items.map((r) => r.value),
                logs: logs.map((r) => r.message),
            });
        },
    },
});
