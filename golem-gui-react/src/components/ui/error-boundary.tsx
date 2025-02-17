import { Alert, Stack } from "@mui/material";
import React, { useState, useEffect } from "react";

interface Props {
  children?: React.ReactNode;
  message?: string | null;
}

const ErrorBoundary: React.FC<Props> = ({ children, message }) => {
  const [hasError, setHasError] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null | undefined>(
    message
  );

  useEffect(() => {
    if (!!message) {
      setErrorMessage(message);
      setHasError(true);
    } else {
      setHasError(false);
    }
  }, [message]);


  console.log("entering this===>", errorMessage)
  if (hasError) {
    return (
      <Stack my={2} alignItems="center">
        <Alert severity="error" color="error">
          {typeof errorMessage !== "string" ? "Something went wrong." : errorMessage }
        </Alert>
        {/* Optionally render children if needed */}
        {children}
      </Stack>
    );
  }

  return children;
};

export default ErrorBoundary;
