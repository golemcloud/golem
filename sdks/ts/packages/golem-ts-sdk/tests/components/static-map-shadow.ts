import { KeyValue, ok, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

class Map {
  constructor() {
    throw new Error('application constructor executed');
  }
}

toolDefinition('static-map-shadow')
  .body((body) =>
    body.option('define', KeyValue(s.s64()), { default: new Map() }).returns(z.string()),
  )
  .implement({ 'static-map-shadow': async () => ok('done') });
