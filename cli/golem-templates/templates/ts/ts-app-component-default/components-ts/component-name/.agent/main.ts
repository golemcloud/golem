import '../.metadata/metadata.index';
import { Metadata } from '@golemcloud/golem-ts-sdk';
import { metadataCollection } from '../.metadata/metadata.index';

// Clear preloaded metadata
Metadata.clearMetadata("@golemcloud/golem-ts-sdk");
// Load generated metadata
metadataCollection.forEach(mod => mod.add(Metadata, false));

// Import the user module after metadata is ready
// This needs to be done this way otherwise rollup ends up generating the module,
// where loading the metadata comes after the user module is loaded - resulting in errors.
export default (async () => {
    return await import("../src/main");
})();
