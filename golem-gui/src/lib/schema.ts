import { z } from 'zod';

// Define the schema for OplogProcessor
const oplogProcessorSchema = z.object({
    type: z.literal('OplogProcessor'),
    name: z.string().nonempty('Plugin name is required'),
    version: z.string().nonempty('Version is required'),
    description: z.string().optional(),
    homepage: z.string().url('Invalid URL').optional(),
    componentId: z.string().nonempty('Component is required'),
    componentVersion: z.number().min(0, 'Version cannot be negative').optional(),
});

// Define the schema for ComponentTransformer
const componentTransformerSchema = z.object({
    type: z.literal('ComponentTransformer'),
    name: z.string().nonempty('Plugin name is required'),
    version: z.string().nonempty('Version is required'),
    description: z.string().optional(),
    homepage: z.string().url('Invalid URL').optional(),
    jsonSchema: z.string().nonempty('JSON Schema is required'),
    validateUrl: z.string().url('Invalid URL'),
    transformUrl: z.string().url('Invalid URL'),
});

// Combine the schemas using a discriminated union
export const pluginSchema = z.discriminatedUnion('type', [
    oplogProcessorSchema,
    componentTransformerSchema,
]);

// Infer TypeScript type from schema
export type PluginFormValues = z.infer<typeof pluginSchema>;
