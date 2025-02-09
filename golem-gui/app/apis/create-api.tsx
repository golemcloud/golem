import React from "react";
import { Box, TextField, Typography } from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import { addNewApiDefinition } from "@/lib/hooks/use-api-definitons";
import { ApiDefinition } from "@/types/api";
import { getFormErrorMessage } from "../../lib/utils";
import { Button2 } from "@/components/ui/button";

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

  const handleCreateAPI = async (data: {
    apiName: string;
    version: string;
  }) => {
    if (isExperimental) {
      return;
    }

    const newApi: ApiDefinition = {
      id: data.apiName,
      version: data.version,
      routes: [],
      createdAt: new Date().toISOString(),
      draft: true,
    };
    const { error } = await addNewApiDefinition(newApi);

    if (error) {
      return;
    }
    onCreation?.();
  };

  return (
    <>
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
        <Box className="mb-2">
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
              <TextField size="small" fullWidth {...field} placeholder="Enter API Name" />
            )}
          />
          <Typography variant="caption" className="text-muted-foreground">Must be unique</Typography>
          <Typography variant="caption" color="error">
            {getFormErrorMessage("apiName", errors)}
          </Typography>
        </Box>
        <Box>
          <Controller
            name="version"
            control={control}
            rules={{
              required: "Version is required",
              pattern: {
                value: /^\d+\.\d+\.\d+$/,
                message:
                  "Version must follow semantic versioning (e.g., 1.0.0)",
              },
            }}
            render={({ field }) => (
              <TextField
                fullWidth
                size="small"
                {...field}
                placeholder="Version"
                
              />
            )}
          />
          <Box>
          <Typography variant="caption" className="text-muted-foreground">Version prefix for your API</Typography>
          <Typography variant="caption" color="error">
            {getFormErrorMessage("version", errors)}
          </Typography>
          </Box>
        </Box>

        {/* Create API Button */}
        <Box display="flex" justifyContent="flex-end" mt={3}>
          <Button2
            type="submit"
            size="lg"
            variant="primary"
            color="primary"
            sx={{ textTransform: "none", fontWeight: "bold" }}
          >
            Create
          </Button2>
        </Box>
      </form>
    </>
  );
};

export default CreateAPI;
