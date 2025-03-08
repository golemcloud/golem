# Ensure the script runs as Administrator
if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")) {
    Write-Output "Please run this script as Administrator!"
    exit 1
}

# Create development directory for storing downloads (without changing location)
New-Item -ItemType Directory -Path C:\dev -Force

# Check if Winget is installed, if not, install it
if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-Output "Winget is not installed. Installing Winget..."

    # Download Winget Installer
    $wingetInstallerUrl = "https://aka.ms/getwinget"
    $wingetInstallerPath = "C:\dev\winget.msixbundle"
    
    Invoke-WebRequest -Uri $wingetInstallerUrl -OutFile $wingetInstallerPath
    
    # Install Winget
    Add-AppxPackage -Path $wingetInstallerPath
    Remove-Item -Path $wingetInstallerPath

    Write-Output "Winget installation complete. Please restart the terminal and re-run the script if necessary."
}

# Detect system architecture
$arch = if ([System.Environment]::Is64BitOperatingSystem) { "win64" } else { "win32" }
Write-Output "Detected architecture: $arch"

# Install Rust using winget
Write-Output "Installing Rust..."
winget install -e --id Rustlang.Rustup --silent --accept-package-agreements
Write-Output "Rust installation complete."

# Install Nginx using winget
Write-Output "Installing Nginx..."
winget install -e --id Nginx.Nginx --silent --accept-package-agreements
Write-Output "Nginx installation complete."

# Install CMake using winget
Write-Output "Installing CMake..."
winget install -e --id Kitware.CMake --silent --accept-package-agreements
Write-Output "CMake installation complete."

# Install Protobuf manually (since it's not available via winget)
Write-Output "Downloading Protobuf..."
$protobufUrl = "https://github.com/protocolbuffers/protobuf/releases/download/v30.0/protoc-30.0-$arch.zip"
$protobufZipPath = "C:\dev\protobuf.zip"
$protobufExtractPath = "C:\dev\protobuf"

Invoke-WebRequest -Uri $protobufUrl -OutFile $protobufZipPath
Expand-Archive -Path $protobufZipPath -DestinationPath $protobufExtractPath -Force

# Update environment variables
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
$newPaths = @("$protobufExtractPath\bin", "C:\Users\$env:USERNAME\.cargo\bin", "C:\Program Files\nginx", "C:\Program Files\CMake\bin")

foreach ($newPath in $newPaths) {
    if ($userPath -notlike "*$newPath*") {
        [System.Environment]::SetEnvironmentVariable("Path", "$userPath;$newPath", "User")
    }
}

Remove-Item -Path $protobufZipPath
Write-Output "Protobuf installation complete."

Write-Output "All installations are complete!"
