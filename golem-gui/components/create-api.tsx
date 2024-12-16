import React, { useState } from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  MenuItem,
  Select,
  InputLabel,
  FormControl,
  Container,
  Paper,
} from "@mui/material";

const CreateAPI = () => {
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");

  const handleCreateAPI = () => {
    console.log("Creating API with:", { apiName, version });
    //add integration
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
