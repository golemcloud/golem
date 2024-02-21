# Use the official NGINX image as the base image
FROM nginx:latest

# Copy custom NGINX configuration file to the container
COPY /gateway/golem-services.conf.template /etc/nginx/golem-services.conf.template

# Expose port 80
EXPOSE 80

# Start NGINX when the container starts
CMD ["nginx", "-g", "daemon off;"]