import React, { useState } from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  Paper,
} from "@mui/material";
import { addNewApiDefinition } from "@/lib/hooks/use-api-definitons";
import { ApiDefinition } from "@/types/api";

const CreateAPI = ({
  onCreation,
  isExperimental,
}: {
  onCreation?: () => void;
  isExperimental?: boolean;
}) => {
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const [error, setError] = useState<string | null>(null);

  const handleCreateAPI = async () => {
    if (isExperimental) {
      return;
    }
    const newApi: ApiDefinition = {
      id: apiName,
      version: version,
      routes: [],
      draft: true,
    };
    const { error } = await addNewApiDefinition(newApi);

    setError(error || null);
    //TODO: Add mutation logic and toast
    onCreation?.();
    return;
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
      {/* <Typography variant="h5" fontWeight="bold" mb={2}>
        Create a new API
      </Typography>
      <Typography variant="body2" mb={3}>
        Export worker functions as a REST API
      </Typography> */}

      {/* API Name Input */}
      <TextField
        label="API Name"
        placeholder="Enter API name"
        fullWidth
        margin="normal"
        value={apiName}
        onChange={(e) => setApiName(e.target.value)}
      />
      <Typography variant="caption">Must be unique per project</Typography>

      {/* Version Input */}
      <TextField
        label="Version"
        placeholder="0.1.0"
        fullWidth
        margin="normal"
        value={version}
        onChange={(e) => setVersion(e.target.value)}
      />
      <Typography variant="caption">Version prefix for your API</Typography>
      {error && <Typography className="text-red-500">{error}</Typography>}

      {/* Create API Button */}
      <Box display="flex" justifyContent="flex-end" mt={3}>
        <Button
          variant="contained"
          color="primary"
          onClick={handleCreateAPI}
          sx={{ textTransform: "none", fontWeight: "bold" }}
        >
          Create
        </Button>
      </Box>
    </Paper>
  );
};

export default CreateAPI;
