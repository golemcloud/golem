# Ensure the script runs as Administrator
# if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")) {
#     Write-Output "Please run this script as Administrator!"
#     exit 1
# }

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

# Install Visual Studio Build Tools using Winget (installs only the installer)
$installerUrl = "https://c2rsetup.officeapps.live.com/c2r/downloadVS.aspx?sku=community&channel=Release&version=VS2022&source=VSLandingPage&cid=2030:e15d0df7dd674811904a95b6a09e9426"
$installerPath = "C:\dev\vs_community.exe"

# Download the installer
Write-Host "Downloading Visual Studio Community installer..."
Invoke-WebRequest -Uri $installerUrl -OutFile $installerPath

# Install Visual Studio with specified workload
Write-Host "Installing Visual Studio Community with C++ workload..."
Start-Process -FilePath $installerPath -ArgumentList `
    "--passive", `
    "--norestart", `
    "--wait", `
    "--add Microsoft.VisualStudio.Workload.NativeDesktop", `
    "--includeRecommended" `
    -Wait

# Clean up
Write-Host "Cleaning up..."
Remove-Item $installerPath

Write-Host "Installation complete!"

# Install Rust using winget
Write-Output "Installing Rust..."
winget install -e --id Rustlang.Rustup --silent --accept-package-agreements
rustup target add wasm32-wasip1
Write-Output "Rust installation complete."

# Install Nginx using winget
Write-Host "Installing Nginx..."
$ngVersion = "nginx-1.27.4"
$ngZip = "C:\dev\nginx.zip"
$ngPath = "C:\dev\nginx"

Invoke-WebRequest -Uri "https://nginx.org/download/$ngVersion.zip" -OutFile $ngZip
Expand-Archive -Path $ngZip -DestinationPath "C:\dev" -Force
Remove-Item -Path $ngZip -Force

if (Test-Path $ngPath) { Remove-Item -Path $ngPath -Recurse -Force }
Rename-Item -Path "C:\dev\$ngVersion" -NewName "nginx"

Write-Output "Nginx installation complete."

# Install Redis for windows
Write-Output "Installing redis for windows...."
$url = "https://github.com/redis-windows/redis-windows/releases/download/7.4.3/Redis-7.4.3-Windows-x64-msys2.zip"
$zip = "$env:TEMP\redis.zip"; $dest = "C:\dev"
Invoke-WebRequest $url -OutFile $zip
$temp = "$env:TEMP\_unzip"; Expand-Archive $zip -DestinationPath $temp -Force
Move-Item "$temp\*" $dest -Force
Remove-Item $temp -Recurse -Force

Write-Output "Redis installeton complete."


# Install Protobuf manually (since it's not available via winget)
Write-Output "Downloading Protobuf..."
$protobufUrl = "https://github.com/protocolbuffers/protobuf/releases/download/v30.0/protoc-30.0-$arch.zip"
$protobufZipPath = "C:\dev\protobuf.zip"
$protobufExtractPath = "C:\dev\protobuf"

Invoke-WebRequest -Uri $protobufUrl -OutFile $protobufZipPath
Expand-Archive -Path $protobufZipPath -DestinationPath $protobufExtractPath -Force

Remove-Item -Path $protobufZipPath
Write-Output "Protobuf installation complete."


# Update environment variables
$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
$newPaths = @("$protobufExtractPath\bin", "C:\Users\$env:USERNAME\.cargo\bin", "$ngPath","C:\dev\Redis-7.4.3-Windows-x64-msys2" )

foreach ($newPath in $newPaths) {
    if ($userPath -notlike "*$newPath*") {
        [System.Environment]::SetEnvironmentVariable("Path", "$userPath;$newPath", "User")
    }
}





Write-Output "All installations are complete!"
