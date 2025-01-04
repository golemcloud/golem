import React, { useState } from "react";
import { Button, TextField, Card, CardContent, Typography, Box } from "@mui/material";

const FunctionInvocationPreview = () => {
  const [formData, setFormData] = useState({ type: "Tuple", items: [{ type: "Str" }], value: [""] });
  const [view, setView] = useState("Form");

  const handleInputChange = (event:React.ChangeEvent<HTMLInputElement>) => {
    const { value } = event.target;
    setFormData((prev) => ({
      ...prev,
      value: [value],
    }));
  };

  return (
    <Box sx={{ p: 4, backgroundColor: "#2c2c2c", minHeight: "100vh", color: "#fff" }}>
      <Box sx={{ mb: 2, display: "flex", gap: 2 }}>
        <Button
          variant={view === "Form" ? "contained" : "outlined"}
          color="primary"
          onClick={() => setView("Form")}
        >
          Form
        </Button>
        <Button
          variant={view === "Preview" ? "contained" : "outlined"}
          color="primary"
          onClick={() => setView("Preview")}
        >
          Preview
        </Button>
      </Box>

      {view === "Form" ? (
        <Card sx={{ backgroundColor: "#1e1e1e", color: "#fff" }}>
          <CardContent>
            <Typography variant="h6" gutterBottom>
              Function Input
            </Typography>
            <TextField
              fullWidth
              variant="outlined"
              label="Value"
              value={formData.value[0]}
              onChange={handleInputChange}
              InputLabelProps={{
                style: { color: "#9cdcfe" },
              }}
              sx={{
                "& .MuiOutlinedInput-root": {
                  "& fieldset": {
                    borderColor: "#9cdcfe",
                  },
                  "&:hover fieldset": {
                    borderColor: "#66afe9",
                  },
                  "&.Mui-focused fieldset": {
                    borderColor: "#0078d7",
                  },
                },
                input: { color: "#fff" },
                mb: 2,
              }}
            />
          </CardContent>
        </Card>
      ) : (
        <Card sx={{ backgroundColor: "#1e1e1e", color: "#fff" }}>
          <CardContent>
            <Typography variant="h6" gutterBottom>
              Preview
            </Typography>
            <Box
              component="pre"
              sx={{
                backgroundColor: "#121212",
                padding: 2,
                borderRadius: 1,
                color: "#9cdcfe",
                overflow: "auto",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
            >
              {JSON.stringify(
                {
                  typ: {
                    type: formData.type,
                    items: formData.items,
                  },
                  value: formData.value,
                },
                null,
                2
              )}
            </Box>
          </CardContent>
        </Card>
      )}
    </Box>
  );
};

export default FunctionInvocationPreview;
