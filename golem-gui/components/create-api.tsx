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

const CreateAPI = ({onCreation}:{onCreation?:()=>void}) => {
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const  [error, setError] = useState("");

  const handleCreateAPI =async() => {
    try{
      const response = await fetcher('?path=api/definitions', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({
          "id": apiName,
          "version": version,
          "routes": [],
          "draft": true
        }),
      })

      if(response.status>300){
        const error = getErrorMessage(response.data);
        setError(error);
        return;
      }

      setError("");
      //TODO: Add mutation logic and toast
      onCreation?.();
      console.log("result===>", response)
      return;
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  }catch(err){
    console.log("Err", err);
    setError("Something went wrong!. please try again");
  }
  };

  return (
    <Container maxWidth="sm" sx={{ mt: 4 }}>
      <Paper elevation={3} sx={{ p: 4, borderRadius: 2 }}>
        <Typography variant="h5" fontWeight="bold" mb={2}>
          Create a new API
        </Typography>
        <Typography variant="body2" color="text.secondary" mb={3}>
          Export worker functions as a REST API
        </Typography>

        {/* Project Dropdown */}

        {/* API Name Input */}
        <TextField
          label="API Name"
          placeholder="Enter API name"
          fullWidth
          margin="normal"
          value={apiName}
          onChange={(e) => setApiName(e.target.value)}
        />
        <Typography variant="caption" color="text.secondary">
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
        <Typography variant="caption" color="text.secondary">
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
            Create API
          </Button>
        </Box>
      </Paper>
    </Container>
  );
};

export default CreateAPI;
