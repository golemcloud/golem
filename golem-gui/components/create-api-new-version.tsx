import React from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  Stack,
} from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { getFormErrorMessage } from "@/lib/utils";

type FormData = {
  version: string;
};

const CreateNewApiVersion = ({
  apiId,
  version,
  isExperimental,
  onSuccess,
}: {
  apiId: string;
  version?: string;
  isExperimental?: boolean;
  onSuccess?: () => void;
}) => {
  const { addNewApiVersionDefinition } = useApiDefinitions(apiId, version);

  // Initialize react-hook-form
  const {
    control,
    handleSubmit,
    formState: { errors },
  } = useForm<FormData>();

  const onSubmit = async (data: FormData) => {
    if (isExperimental) return; // Block submission if experimental
    await addNewApiVersionDefinition(
      { version: data.version },
      apiId,
      version,
      isExperimental
    );
    onSuccess?.(); // Call success callback if provided
  };

  return (
    <Box sx={{ p: 2 }}>
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

      <form onSubmit={handleSubmit(onSubmit)}>
        {/* API Version Input */}
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
              label="Version"
              placeholder="Enter API version (e.g., 1.0.0)"
              fullWidth
              margin="normal"
            />
          )}
        />
        <Typography mb={3} variant="caption">
          Create new version from API <strong>{version}</strong>
        </Typography>
        <Typography variant="caption" color="error">
                    {getFormErrorMessage("version", errors)}
                  </Typography>

        {/* Submit Button */}
        <Stack>
          <Button
            type="submit"
            className="self-end"
            variant="contained"
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
