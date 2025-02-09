import React from "react";
import { Box, TextField, Typography, Stack } from "@mui/material";
import { useForm, Controller } from "react-hook-form";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { getFormErrorMessage } from "@/lib/utils";
import { Button2 as Button } from "@/components/ui/button";
import { ApiVersionProps } from "./types";

type FormData = {
  version: string;
};
type DeleteApiProps = ApiVersionProps & {
  onSuccess?: () => void;
};

const DeleteApiVersion = ({
  apiId,
  version,
  isExperimental,
  noRedirect,
  onSuccess,
}: DeleteApiProps ) => {
  const { deleteVersion } = useApiDefinitions(apiId);

  const {
    control,
    handleSubmit,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: {
      version: "",
    },
  });

  const onSubmit = async (data: FormData) => {
    if (isExperimental) return;
    await deleteVersion(apiId, data.version, noRedirect);
    onSuccess?.();
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
