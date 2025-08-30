import { TypescriptTypeRegistry } from '@golemcloud/golem-ts-sdk';
import { Metadata } from '../../../.metadata/generated-types';

TypescriptTypeRegistry.register(Metadata);

// Import the user module after metadata is ready
// This needs to be done this way otherwise rollup ends up generating the module,
// where loading the metadata comes after the user module is loaded - resulting in errors.
export default (async () => {
    return await import("../src/main");
})();
