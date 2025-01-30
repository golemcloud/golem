import React, { useState } from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  MenuItem,
  Select,
  TextField,
  CircularProgress,
} from "@mui/material";
import { useAddPlugin } from "@/lib/hooks/use-plugin";
import useComponents from "@/lib/hooks/use-component";
import { Component, Plugin } from "@/types/api";
import { zodResolver } from "@hookform/resolvers/zod";
import { pluginSchema, PluginFormValues } from "@/lib/schema";
import { Button2 } from "./ui/button";

const CreatePluginForm = () => {
  const { components } = useComponents();
  const { upsertPulgin } = useAddPlugin();
  console.log("components ", components);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const {
    handleSubmit,
    control,
    watch,
    formState: { errors },
  } = useForm<PluginFormValues>({
    resolver: zodResolver(pluginSchema),
    defaultValues: {
      name: "",
      version: "",
      description: "",
      homepage: "",
      type: "ComponentTransformer",
      jsonSchema: "",
      validateUrl: "",
      transformUrl: "",
    },
  });

  const pluginType = watch("type");
  const selectedComponentId = watch("componentId");

  const seenIds = new Set();
  const uniqueOptions = components.filter((component: Component) => {
    const id = component.versionedComponentId?.componentId;
    if (seenIds.has(id)) {
      return false;
    }
    seenIds.add(id);
    return true;
  });
  const onSubmit = async (data: PluginFormValues) => {
    setIsSubmitting(true);
    const pluginData = {
      name: data.name,
      version: data.version,
      description: data.description,
      specs:
        data.type === "OplogProcessor"
          ? {
              type: "OplogProcessor",
              componentId: data.componentId,
              componentVersion: (data.componentVersion),
            }
          : {
              type: "ComponentTransformer",
              jsonSchema: data.jsonSchema,
              validateUrl: data.validateUrl,
              transformUrl: data.transformUrl,
            },
      scope: { type: "Global" },
      icon: [0],
      homepage: data.homepage,
    } as Plugin;

    await upsertPulgin(pluginData);
    setIsSubmitting(false);
  };

  console.log("error===>", errors);

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      <Box display="grid" className="md:grid-cols-2 grid-cols-1" gap={2}>
        {/* Plugin Name */}
        <Controller
          name="name"
          control={control}
          render={({ field, fieldState }) => (
            <TextField
              {...field}
              size="small"
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
              size="small"
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
            <Select {...field} size="small" fullWidth disabled={isSubmitting}>
              <MenuItem value="OplogProcessor">Oplog Processor</MenuItem>
              <MenuItem value="ComponentTransformer">
                Component Transformer
              </MenuItem>
            </Select>
          )}
        />

        {/* Conditional Fields */}
        {pluginType === "OplogProcessor" && (
          <>
            <Controller
              name="componentId"
              control={control}
              render={({ field, fieldState }) => (
                <TextField
                  {...field}
                  select
                  label="Component"
                  size="small"
                  error={!!fieldState.error}
                  helperText={fieldState.error?.message}
                  disabled={isSubmitting}
                  fullWidth
                >
                  {uniqueOptions?.map((component: Component) => (
                    <MenuItem
                      key={component.versionedComponentId.componentId}
                      value={component.versionedComponentId.componentId}
                    >
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
                  select
                  label="Component Version"
                  size="small"
                  error={!!fieldState.error}
                  helperText={fieldState.error?.message}
                  disabled={isSubmitting}
                  fullWidth
                >
                  {components
                    ?.filter(
                      (component) =>
                        component.versionedComponentId.componentId ===
                        selectedComponentId
                    )
                    ?.map((component) => (
                      <MenuItem
                        key={component.versionedComponentId.version}
                        value={component.versionedComponentId.version}
                      >
                        {component.versionedComponentId.version}
                      </MenuItem>
                    ))}
                </TextField>
              )}
            />
          </>
        )}

        {pluginType === "ComponentTransformer" && (
          <>
            <Controller
              name="jsonSchema"
              control={control}
              render={({ field, fieldState }) => (
                <TextField
                  {...field}
                  size="small"
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
              size="small"
              error={!!fieldState.error}
              helperText={fieldState.error?.message}
              disabled={isSubmitting}
              fullWidth
            />
          )}
        />
      </Box>
      <Box mt={4} display="flex" justifyContent="flex-end">
        <Button2
          type="submit"
          variant="primary"
          size="lg"
          disabled={isSubmitting}
        >
          {isSubmitting ? <CircularProgress size={24} /> : "Create Plugin"}
        </Button2>
      </Box>
    </form>
  );
};

export default CreatePluginForm;
