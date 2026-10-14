import { ok, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

toolDefinition('greet')
  .body((body) => body.positional('name', z.string()).returns(z.string()))
  .implement({ greet: ({ name }) => ok(`Hello, ${name}!`) });
