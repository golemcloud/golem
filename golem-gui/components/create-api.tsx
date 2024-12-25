import React from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  Paper,
} from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import { addNewApiDefinition } from "@/lib/hooks/use-api-definitons";
import { ApiDefinition } from "@/types/api";
import { toast } from "react-toastify";
import { getFormErrorMessage } from '../lib/utils';

const CreateAPI = ({
  onCreation,
  isExperimental,
}: {
  onCreation?: () => void;
  isExperimental?: boolean;
}) => {
  const {
    control,
    handleSubmit,
    formState: { errors },
  } = useForm({
    defaultValues: {
      apiName: "",
      version: "0.1.0",
    },
  });

  const handleCreateAPI = async (data: { apiName: string; version: string }) => {
    if (isExperimental) {
      return;
    }

    const newApi: ApiDefinition = {
      id: data.apiName,
      version: data.version,
      routes: [],
      draft: true,
    };
    const { error } = await addNewApiDefinition(newApi);

    if (error) {
      return toast.error(`Api Creation failed! ${error}`)
    }
    toast.success(`Successfully created Api!`)
    onCreation?.();
  };

  return (
    <Paper elevation={4} sx={{ p: 2 }}>
      {isExperimental && (
        <Typography
          variant="h5"
          fontWeight="bold"
          mb={2}
          className="text-red-500 text-center"
        >
          Experimental. Coming soon!
        </Typography>
      )}

      <form onSubmit={handleSubmit(handleCreateAPI)}>
        {/* API Name Input */}
        <Controller
          name="apiName"
          control={control}
          rules={{
            required: "API Name is required",
            maxLength: {
              value: 50,
              message: "API Name cannot exceed 50 characters",
            },
          }}
          render={({ field }) => (
            <TextField
              {...field}
              label="API Name"
              placeholder="Enter API name"
              fullWidth
              margin="normal"
            />
          )}
        />
        <Typography variant="caption">Must be unique per project</Typography>
        <Typography variant="caption" color="error">{getFormErrorMessage("apiName", errors)}</Typography>

        {/* Version Input */}
        <Controller
          name="version"
          control={control}
          rules={{
            required: "Version is required",
            pattern: {
              value: /^\d+\.\d+\.\d+$/,
              message: "Version must follow semantic versioning (e.g., 1.0.0)",
            },
          }}
          render={({ field }) => (
            <TextField
              {...field}
              label="Version"
              placeholder="0.1.0"
              fullWidth
              margin="normal"
            />
          )}
        />
        <Typography variant="caption">Version prefix for your API</Typography>
        <Typography variant="caption" color="error">{getFormErrorMessage("version", errors)}</Typography>

        {/* Create API Button */}
        <Box display="flex" justifyContent="flex-end" mt={3}>
          <Button
            type="submit"
            variant="contained"
            color="primary"
            sx={{ textTransform: "none", fontWeight: "bold" }}
          >
            Create
          </Button>
        </Box>
      </form>
    </Paper>
  );
};

export default CreateAPI;

