import React, { useState } from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  Paper,
  Stack,
} from "@mui/material";
import useApiDefinitions, {
  addNewApiDefinition,
} from "@/lib/hooks/use-api-definitons";
import { ApiDefinition } from "@/types/api";

const CreateNewApiVersion = ({
  apiId,
  version,
  isExperimental,
  onSuccees,
}: {
  apiId: string;
  version?: string;
  isExperimental?: boolean;
  onSuccees?: () => void;
}) => {
  const [newVersion, setNewVersion] = useState("");
  const { addNewApiVersionDefinition } = useApiDefinitions(apiId, version);
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
      {/* <Typography variant="h5" fontWeight="bold" mb={2}>
        Create a new API
      </Typography>
      <Typography variant="body2" mb={3}>
        Export worker functions as a REST API
      </Typography> */}

      {/* API Name Input */}
      <TextField
        placeholder="Version"
        name="version"
        label="version"
        required
        onChange={(e) => {
          setNewVersion(e.target.value);
        }}
      />
      <Typography>Create new version from api {version}</Typography>
      <Stack>
        <Button
          onClick={async (e) => {
            e.preventDefault();
            if(isExperimental){
                return;
            }
            await addNewApiVersionDefinition(
              { version: newVersion },
              apiId,
              version
            );
            onSuccees?.();
          }}
          className="self-end"
        >
          Create New
        </Button>
      </Stack>
    </Paper>
  );
};

export default CreateNewApiVersion;
