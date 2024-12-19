export interface GolemError {
  error?: string;
  errors?: string[];
  type?: string;
  golemError?: {
    type: string;
    details: string;
  };
}

export const getErrorMessage = (error: GolemError): string => {
  if (error.golemError) {
    return `${error.golemError.type}: ${error.golemError.details}`;
  }

  if (error.errors?.length) {
    return error.errors.join(", ");
  }

  if (error.error) {
    return error.error;
  }

  return "An unknown error occurred";
};
