import React from "react";
import { Box, TextField, Typography, Stack } from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { getFormErrorMessage } from "@/lib/utils";
import {Button2 as Button} from "./ui/button";

type FormData = {
  version: string;
};

const CreateNewApiVersion = ({
  apiId,
  version,
  isExperimental,
  noRedirect,
  onSuccess,
}: {
  apiId: string;
  version?: string|null;
  isExperimental?: boolean;
  noRedirect?: boolean;
  onSuccess?: (data:{version: string}) => void;
}) => {
  const { addNewApiVersionDefinition } = useApiDefinitions(apiId, version);

  // Initialize react-hook-form
  const {
    control,
    handleSubmit,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: {
      version: "", // Default to an empty string
    },
  });

  const onSubmit = async (data: FormData) => {
    if (isExperimental) return; // Block submission if experimental
    await addNewApiVersionDefinition(
      { version: data.version },
      apiId,
      version,
      noRedirect,
    );
    onSuccess?.({ version: data.version }); // Call success callback if provided
  };

  return (
    <Box>
      {isExperimental && (
        <Typography
          variant="h5"
          fontWeight="bold"
          mb={2}
          className="text-red-600 text-center border border-red-300 rounded-lg shadow-lg font-thin"
        >
          Experimental. Coming soon!
        </Typography>
      )}

      <form onSubmit={handleSubmit(onSubmit)}>
        {/* API Version Input */}
        <Box className="mb-2">
          <Typography className="text-foreground">New Version</Typography>
          <Controller
            name="version"
            control={control}
            rules={{
              required: "Version is required",
              pattern: {
                value: /^[0-9]+\.[0-9]+\.[0-9]+$/, // Semantic version pattern
                message: "Version must be in semantic format (e.g., 1.0.0)",
              },
            }}
            render={({ field }) => (
              <TextField
                {...field}
                size="small"
                placeholder="Enter API version (e.g., 1.0.0)"
                fullWidth
              />
            )}
          />
          <p className="text-muted-foreground text-xs mt-1">Create new version from API <strong>{version}</strong></p>
          <Typography variant="caption" color="error">
            {getFormErrorMessage("version", errors)}
          </Typography>
        </Box>

        {/* Submit Button */}
        <Stack>
          <Button
            type="submit"
            className="self-end"
            variant="primary"
            size="md"
            color={isExperimental ? "error" : "primary"}
            disabled={isExperimental}
          >
            {isExperimental ? "Experimental Feature" : "Create New"}
          </Button>
          </Stack>
      </form>
    </Box>
  );
};

export default CreateNewApiVersion;
