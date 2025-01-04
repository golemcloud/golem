import React from "react";
import { Box, TextField, Typography, Stack } from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { getFormErrorMessage } from "@/lib/utils";
import {Button2 as Button} from "./ui/button";

type FormData = {
  version: string;
};

const DeleteApiVersion = ({
  apiId,
  version,
  isExperimental,
  onSuccess,
}: {
  apiId: string;
  version?: string|null;
  isExperimental?: boolean;
  onSuccess?: () => void;
}) => {
  const { deleteVersion } = useApiDefinitions(apiId);

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
    await deleteVersion(apiId, data.version);
    onSuccess?.(); // Call success callback if provided
  };

  return (
    <Box sx={{ p: 2 }}>
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
        <Controller
          name="version"
          control={control}
          rules={{
            required: "Version is required",
            pattern: {
              value: /^[0-9]+\.[0-9]+\.[0-9]+$/, // Semantic version pattern
              message: "Version must be in semantic format (e.g., 1.0.0)",
            },
            validate: (value) => {
                return version && value !== version ? "Version does not match" : true;
            }
          }}
          render={({ field }) => (
            <TextField
              {...field}
              label="Version"
              size="small"
              placeholder={`Enter API version ${version}`}
              fullWidth
              margin="normal"
            />
          )}
        />
        <Stack>
        <Typography  variant="caption">
          Delete version from API <strong>{version}</strong>
        </Typography>
        <Typography mb={3} variant="caption" color="error">
          {getFormErrorMessage("version", errors)}
        </Typography>
        </Stack>

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
            {isExperimental ? "Experimental Feature" : "Delete Version"}
          </Button>
        </Stack>
      </form>
    </Box>
  );
};

export default DeleteApiVersion;
