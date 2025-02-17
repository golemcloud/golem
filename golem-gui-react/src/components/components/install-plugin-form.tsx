
import React, { useState } from "react";
import { useForm, Controller } from "react-hook-form";
import { Box, TextField, CircularProgress, Typography } from "@mui/material";
import { Button2 } from "@ui/button";
import { installPlugin, useUninstallPlugin } from "@lib/hooks/use-component";
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { Plugin } from "@lib/types/api";
interface PluginFormData {
  name: string;
  version: string;
  priority: number;
  parameters: {
    property1: string;
    property2: string;
  };
}

const PluginInstallForm = ({ onSuccess }: { onSuccess?: () => void }) => {
  const [isSubmitting, setIsSubmitting] = useState(false);
  const {compId}=useCustomParam()
  const {
    handleSubmit,
    control,
  } = useForm<PluginFormData>({
    defaultValues: {
      name: "",
      version: "",
      priority: 0,
      parameters: {
        property1: "",
        property2: "",
      },
    },
  });
  
  const onSubmit = async (data: PluginFormData) => {
    setIsSubmitting(true);
    const formattedData = {
      ...data,
      priority: Number(data.priority),
    };
    const resp=await installPlugin(formattedData,compId);
    console.log("resp",resp);
    setTimeout(() => setIsSubmitting(false), 1000);
    onSuccess?.();
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      <Box display="grid" className="md:grid-cols-2 grid-cols-1" gap={2}>
        {/* Name */}
        <Controller
          name="name"
          control={control}
          render={({ field, fieldState }) => (
            <TextField
              {...field}
              size="small"
              label="Name"
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
              size="small"
              label="Version"
              error={!!fieldState.error}
              helperText={fieldState.error?.message}
              disabled={isSubmitting}
              fullWidth
            />
          )}
        />

        {/* Priority */}
        <Controller
          name="priority"
          control={control}
          render={({ field, fieldState }) => (
            <TextField
              {...field}
              type="number"
              size="small"
              label="Priority"
              error={!!fieldState.error}
              helperText={fieldState.error?.message}
              disabled={isSubmitting}
              fullWidth
            />
          )}
        />

        {/* Parameters - Property 1 */}
        <Controller
          name="parameters.property1"
          control={control}
          render={({ field, fieldState }) => (
            <TextField
              {...field}
              size="small"
              label="Property 1"
              error={!!fieldState.error}
              helperText={fieldState.error?.message}
              disabled={isSubmitting}
              fullWidth
            />
          )}
        />

        {/* Parameters - Property 2 */}
        <Controller
          name="parameters.property2"
          control={control}
          render={({ field, fieldState }) => (
            <TextField
              {...field}
              size="small"
              label="Property 2"
              error={!!fieldState.error}
              helperText={fieldState.error?.message}
              disabled={isSubmitting}
              fullWidth
            />
          )}
        />
      </Box>
      <Box mt={4} display="flex" justifyContent="flex-end">
        <Button2 type="submit" variant="primary" size="lg" disabled={isSubmitting}>
          {isSubmitting ? <CircularProgress size={24} /> : "Create"}
        </Button2>
      </Box>
    </form>
  );
};

export const PluginUninstallForm = ({
  plugin,
  onSuccess,
}: {
  plugin: Plugin|null;
  onSuccess?: () => void;
}) => {
  const {compId}=useCustomParam();

  const { uninstallPlugin } = useUninstallPlugin(compId);
 console.log("plugin",plugin)
  const handleUninstall = async () => {
    // @ts-expect-error - The structure of `plugin` is not fully typed yet
    await uninstallPlugin(plugin.id);
    if (onSuccess) onSuccess();
  };

  return (
    <Box display="flex" flexDirection="column" gap={2}>
      <Typography>Are you sure you want to uninstall {plugin?.name}?</Typography>
      <Box display="flex" justifyContent="flex-end">
        <Button2 variant="error" onClick={handleUninstall}>
          Uninstall
        </Button2>
      </Box>
    </Box>
  );
};

export default PluginInstallForm;
