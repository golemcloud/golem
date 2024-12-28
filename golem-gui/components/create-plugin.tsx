import React, { useState } from 'react';
import { useForm, Controller } from 'react-hook-form';
import {
    Box,
    Button,
    MenuItem,
    Paper,
    Select,
    TextField,
    Typography,
    CircularProgress,
} from '@mui/material';
import { useAddPlugin } from '@/lib/hooks/use-plugin';
import useComponents from '@/lib/hooks/use-component';
import { Component, Plugin } from '@/types/api';
import { zodResolver } from '@hookform/resolvers/zod';
import {pluginSchema, PluginFormValues} from '@/lib/schema'


const CreatePluginForm = () => {
    const { components } = useComponents();
    const {upsertPulgin} = useAddPlugin();

    const [isSubmitting, setIsSubmitting] = useState(false);

    const { handleSubmit, control, watch, formState: { errors } } = useForm<PluginFormValues>({
        resolver: zodResolver(pluginSchema),
        defaultValues: {
            name: '',
            version: '',
            description: '',
            homepage: '',
            type: 'ComponentTransformer',
            jsonSchema: '',
            validateUrl: '',
            transformUrl: '',
        },
    });

    const pluginType = watch('type');

    const onSubmit = async (data: PluginFormValues) => {
        setIsSubmitting(true);
        const pluginData = {
            name: data.name,
            version: data.version,
            description: data.description,
            specs:
                data.type === 'OplogProcessor'
                    ? { type: 'OplogProcessor', componentId: data.componentId, componentVersion: data.componentVersion }
                    : { type: 'ComponentTransformer', jsonSchema: data.jsonSchema, validateUrl: data.validateUrl, transformUrl: data.transformUrl },
            scope: { type: 'Global' },
            icon: [0],
            homepage: data.homepage,
        } as Plugin;

        await upsertPulgin(pluginData)
        setIsSubmitting(false);
    };

    console.log("error===>", errors)

    return (
        <>
        <Paper elevation={3} sx={{ padding: 4, maxWidth: 800, margin: 'auto', mt: 4 }}>
            <Typography variant="h5" gutterBottom>
                Create New Plugin
            </Typography>
            <form onSubmit={handleSubmit(onSubmit)}>
                <Box display="grid" gridTemplateColumns="1fr 1fr" gap={2}>
                    {/* Plugin Name */}
                    <Controller
                        name="name"
                        control={control}
                        render={({ field, fieldState }) => (
                            <TextField
                                {...field}
                                label="Plugin Name"
                                error={!!fieldState.error}
                                helperText={fieldState.error?.message}
                                disabled={isSubmitting}
                                fullWidth
                            />
                        )}
                    />

                    {/* Version */}
                    <Controller
                        name="version"
                        control={control}
                        render={({ field, fieldState }) => (
                            <TextField
                                {...field}
                                type="number"
                                label="Version"
                                error={!!fieldState.error}
                                helperText={fieldState.error?.message}
                                disabled={isSubmitting}
                                fullWidth
                            />
                        )}
                    />

                    {/* Type */}
                    <Controller
                        name="type"
                        control={control}
                        render={({ field }) => (
                            <Select {...field} label="Plugin Type" fullWidth disabled={isSubmitting}>
                                <MenuItem value="OplogProcessor">OplogProcessor</MenuItem>
                                <MenuItem value="ComponentTransformer">ComponentTransformer</MenuItem>
                            </Select>
                        )}
                    />

                    {/* Conditional Fields */}
                    {pluginType === 'OplogProcessor' && (
                        <>
                            <Controller
                                name="componentId"
                                control={control}
                                render={({ field, fieldState }) => (
                                    <TextField
                                        {...field}
                                        select
                                        label="Component"
                                        error={!!fieldState.error}
                                        helperText={fieldState.error?.message}
                                        disabled={isSubmitting}
                                        fullWidth
                                    >
                                        {components?.map((component: Component) => (
                                            <MenuItem key={component.versionedComponentId.componentId} value={component.versionedComponentId.componentId}>
                                                {component.componentName}
                                            </MenuItem>
                                        ))}
                                    </TextField>
                                )}
                            />

                            <Controller
                                name="componentVersion"
                                control={control}
                                render={({ field, fieldState }) => (
                                    <TextField
                                        {...field}
                                        label="Component Version"
                                        type="number"
                                        error={!!fieldState.error}
                                        helperText={fieldState.error?.message}
                                        disabled={isSubmitting}
                                        fullWidth
                                        onChange={(e) => field.onChange(Number(e.target.value))}
                                    />
                                )}
                            />
                        </>
                    )}

                    {pluginType === 'ComponentTransformer' && (
                        <>
                            <Controller
                                name="jsonSchema"
                                control={control}
                                render={({ field, fieldState }) => (
                                    <TextField
                                        {...field}
                                        label="JSON Schema"
                                        error={!!fieldState.error}
                                        helperText={fieldState.error?.message}
                                        disabled={isSubmitting}
                                        fullWidth
                                    />
                                )}
                            />
                        </>
                    )}

                    {/* Other Fields */}
                    <Controller
                        name="homepage"
                        control={control}
                        render={({ field, fieldState }) => (
                            <TextField
                                {...field}
                                label="Homepage"
                                error={!!fieldState.error}
                                helperText={fieldState.error?.message}
                                disabled={isSubmitting}
                                fullWidth
                            />
                        )}
                    />
                </Box>
                <Box mt={4} display="flex" justifyContent="flex-end">
                    <Button type="submit" variant="contained" color="primary" disabled={isSubmitting}>
                        {isSubmitting ? <CircularProgress size={24} /> : 'Create Plugin'}
                    </Button>
                </Box>
            </form>
        </Paper>
        </>
    );
};

export default CreatePluginForm;
