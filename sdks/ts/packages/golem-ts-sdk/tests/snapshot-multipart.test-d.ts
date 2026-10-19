import { z } from 'zod';
import { defineAgent } from '../src/defineAgent';

const def = defineAgent({
  name: 'MultipartTypes',
  id: {},
  methods: {},
  snapshotting: { multipart: { state: z.object({ revision: z.string().transform(Number) }) } },
});

def.implement({
  init: () => ({ counter: 0, index: new Uint8Array() }),
  methods: {},
  snapshot: {
    save() {
      return { state: { revision: String(this.counter) }, parts: new Map() };
    },
    load(saved) {
      const revision: number = saved.state.revision;
      // @ts-expect-error restoration receives transformed output, not input
      const input: string = saved.state.revision;
      void input;
      return { counter: revision, index: new Uint8Array() };
    },
  },
});

// @ts-expect-error multipart requires both projection and restoration
def.implement({ init: () => ({}), methods: {} });
def.implement({
  init: () => ({}),
  methods: {},
  // @ts-expect-error incomplete pair
  snapshot: { save: () => ({ state: { revision: '7' }, parts: new Map() }) },
});
def.implement({
  init: () => ({}),
  methods: {},
  // @ts-expect-error multipart cannot omit its projection
  snapshot: { load: () => ({}) },
});
def.implement({
  init: () => ({}),
  methods: {},
  snapshot: {
    // @ts-expect-error saver returns schema input, not transformed output
    save: () => ({ state: { revision: 7 }, parts: new Map() }),
    load: () => ({}),
  },
});
// @ts-expect-error ordinary state and multipart are mutually exclusive
defineAgent({
  name: 'ConflictingMultipartTypes',
  id: {},
  methods: {},
  snapshotting: { state: z.null(), multipart: { state: z.null() } },
});
// @ts-expect-error multipart.state is required
defineAgent({
  name: 'MissingMultipartState',
  id: {},
  methods: {},
  snapshotting: { multipart: {} },
});
