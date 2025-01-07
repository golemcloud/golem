# Create the OpenAPI Generator directory if it doesn't exist
$openApiDir = "$env:USERPROFILE\.openapi-generator"
New-Item -ItemType Directory -Force -Path $openApiDir | Out-Null

# Download the latest version of OpenAPI Generator CLI
$jarFile = "$openApiDir\openapi-generator-cli.jar"
$url = "https://repo1.maven.org/maven2/org/openapitools/openapi-generator-cli/6.6.0/openapi-generator-cli-6.6.0.jar"
Write-Host "Downloading OpenAPI Generator CLI..."
Invoke-WebRequest -Uri $url -OutFile $jarFile

# Create a wrapper script
$wrapperScript = @"
java -jar "$env:USERPROFILE\.openapi-generator\openapi-generator-cli.jar" `$args
"@
Set-Content -Path "$openApiDir\openapi-generator-cli.ps1" -Value $wrapperScript

Write-Host "OpenAPI Generator CLI has been installed successfully!"
Write-Host "You can now use 'openapi-generator-cli' to generate clients." 