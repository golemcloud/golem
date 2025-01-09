import axios from "axios";

// Create axios instance with default config
export const apiClient = axios.create({
  // baseURL: "/api",
  baseURL: "http://localhost:3000/api",
  // baseURL: "http://localhost:9881",
  headers: {
    "Content-Type": "application/json",
  },
});

// Add response interceptor for error handling
apiClient.interceptors.response.use(
  (response) => response,
  (error) => {
    // Handle different error types
    if (error.response) {
      // Server responded with non-2xx status
      console.error("API Error:", error.response.data);
      return Promise.reject(error.response.data);
    } else if (error.request) {
      // Request made but no response received
      console.error("Network Error:", error.request);
      return Promise.reject(new Error("Network error occurred"));
    } else {
      // Error in request setup
      console.error("Request Error:", error.message);
      return Promise.reject(error);
    }
  },
);
