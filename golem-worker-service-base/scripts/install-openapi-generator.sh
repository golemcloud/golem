#!/bin/bash

# Determine the latest version of OpenAPI Generator CLI
VERSION=$(curl -s https://api.github.com/repos/OpenAPITools/openapi-generator/releases/latest | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

# Create the directory for the JAR file
mkdir -p ~/.openapi-generator

# Download the JAR file
curl -o ~/.openapi-generator/openapi-generator-cli.jar https://repo1.maven.org/maven2/org/openapitools/openapi-generator-cli/${VERSION}/openapi-generator-cli-${VERSION}.jar

# Create a wrapper script
cat > /usr/local/bin/openapi-generator-cli << 'EOF'
#!/bin/bash
java -jar ~/.openapi-generator/openapi-generator-cli.jar "$@"
EOF

# Make the wrapper script executable
chmod +x /usr/local/bin/openapi-generator-cli

echo "OpenAPI Generator CLI installed successfully!" 