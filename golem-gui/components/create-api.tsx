import React, { useState } from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  Container,
  Paper,
} from "@mui/material";
import { fetcher, getErrorMessage } from "@/lib/utils";

const CreateAPI = ({
  onCreation,
  isExperimental,
}: {
  onCreation?: () => void;
  isExperimental?: boolean;
}) => {
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const [error, setError] = useState("");

  const handleCreateAPI = async () => {
    if (isExperimental) {
      return;
    }
    try {
      const response = await fetcher("?path=api/definitions", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          id: apiName,
          version: version,
          routes: [],
          draft: true,
        }),
      });

      if (response.status > 300) {
        const error = getErrorMessage(response.data);
        setError(error);
        return;
      }

      setError("");
      //TODO: Add mutation logic and toast
      onCreation?.();
      console.log("result===>", response);
      return;
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
    } catch (err) {
      console.log("Err", err);
      setError("Something went wrong!. please try again");
    }
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
      <Typography variant="caption">
        Must be unique per project
      </Typography>

      {/* Version Input */}
      <TextField
        label="Version"
        placeholder="0.1.0"
        fullWidth
        margin="normal"
        value={version}
        onChange={(e) => setVersion(e.target.value)}
      />
      <Typography variant="caption" >
        Version prefix for your API
      </Typography>
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
