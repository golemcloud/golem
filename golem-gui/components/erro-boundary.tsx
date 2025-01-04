import { Alert, Stack } from "@mui/material";
import React, { useState, useEffect, ErrorInfo } from "react";

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

  const handleError = (error: Error, errorInfo: ErrorInfo) => {
    console.error("ErrorBoundary caught an error: ", error, errorInfo);
    setErrorMessage(error.message);
    setHasError(true);
  };

  if (hasError) {
    return (
      <Stack my={2} alignItems="center">
        <Alert severity="error">
          {errorMessage || "Something went wrong."}
        </Alert>
        {/* Optionally render children if needed */}
        {children}
      </Stack>
    );
  }

  return children;
};

export default ErrorBoundary;
