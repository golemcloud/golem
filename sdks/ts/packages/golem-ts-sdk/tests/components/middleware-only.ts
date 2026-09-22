import { toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

toolDefinition('greet')
  .body((body) => body.positional('name', z.string()).returns(z.string()))
  .middleware({
    name: 'greeting-policy',
    implementation: { greet: ({ name }) => `Welcome, ${name}!` },
  });
