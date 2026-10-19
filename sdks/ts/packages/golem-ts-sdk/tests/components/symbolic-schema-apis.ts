import { durable, registeredVendors, type Bucket, type Container } from '@golemcloud/golem-ts-sdk';

export const useDurable = durable;
export const useBucketSchema = (bucket: Bucket, schema: any) => bucket.forSchema(schema);
export const useContainerSchema = (container: Container, schema: any) =>
  container.forSchema(schema);

(globalThis as any).__golemRegisteredSchemaVendors = () => registeredVendors();
