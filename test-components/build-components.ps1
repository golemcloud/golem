<#
.SYNOPSIS
    Builds Golem test components for various languages.

.DESCRIPTION
    This script compiles and packages WebAssembly components from source code written in Rust, Zig, TinyGo, Grain, JS, Java, .NET, Swift, C, and Python.
    It can build all components or target a specific language. It also provides options to clean previous builds and update WIT dependencies.

.PARAMETER rebuild
    If specified, cleans all projects before building them.

.PARAMETER updateWit
    If specified, updates the wit/deps directories for projects that have a wit/deps.toml file.

.PARAMETER lang
    Builds only the components for the specified language.

.EXAMPLE
    .\build-components.ps1
    Builds all test components for all languages.

.EXAMPLE
    .\build-components.ps1 -lang rust
    Builds only the Rust test components and apps.

.EXAMPLE
    .\build-components.ps1 -rebuild -updateWit
    Cleans all projects, updates their WIT dependencies, and then builds them.
#>
param(
    [switch]$rebuild,

    [switch]$updateWit,

    [ValidateSet('rust', 'zig', 'tinygo', 'grain', 'js', 'java', 'dotnet', 'swift', 'c', 'python', 'ts')]
    [string]$lang
)

$ErrorActionPreference = "Stop"

# $rust_test_components = @("write-stdout", "write-stderr", "read-stdin", "clocks", "shopping-cart", "file-write-read-delete", "file-service", "http-client", "directories", "environment-service", "promise", "interruption", "clock-service", "option-service", "flags-service", "http-client-2", "stdio-cc", "failing-component", "variant-service", "key-value-service", "blob-store-service", "runtime-service", "networking", "shopping-cart-resource", "update-test-v1", "update-test-v2-11", "update-test-v3-11", "update-test-v4", "rust-echo", "durability-overhead", "logging", "oplog-processor", "rdbms-service", "component-resolve", "golem-rust-tests")
$rust_test_components = @("golem-rust-tests")
$zig_test_components = @("zig-3")
$tinygo_test_components = @("tinygo-wasi", "tinygo-wasi-http")
$grain_test_components = @("grain-1")
$js_test_components = @("js-1", "js-2", "js-3", "js-4", "js-echo")
$java_test_components = @("java-1", "java-2")
$dotnet_test_components = @("csharp-1")
$swift_test_components = @("swift-1")
$c_test_components = @("c-1", "large-initial-memory", "large-dynamic-memory")
$python_test_components = @("python-1", "py-echo")

$rust_test_apps = @("auction-example", "rpc", "rust-service/rpc")
$ts_test_apps = @("ts-rpc")

$single_lang = $PSBoundParameters.ContainsKey('lang')

if (-not $single_lang -or $lang -eq 'rust') {
    Write-Host "Building the Rust test components" -ForegroundColor Green
    foreach ($subdir in $rust_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            & cargo clean
        }
        & cargo component build --release

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        $sourceWasm = Get-ChildItem -Path "target/wasm32-wasip1/release" -Filter "*.wasm" -File | Select-Object -First 1
        Copy-Item -Path $sourceWasm.FullName -Destination $target -Verbose
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'rust') {
    Write-Host "Building the Rust test apps" -ForegroundColor Green
    foreach ($subdir in $rust_test_apps) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            & golem-cli app clean
            & cargo clean
        }

        & golem-cli app -b release build
        & golem-cli app -b release copy

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'zig') {
    Write-Host "Building the Zig test components" -ForegroundColor Green
    foreach ($subdir in $zig_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "zig-out", "zig-cache" -Recurse -Force -ErrorAction SilentlyContinue
        }
        & zig build -Dadapter=../../../golem-wit/adapters/tier1/wasi_snapshot_preview1.wasm

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        Copy-Item -Path "zig-out/bin/component.wasm" -Destination $target
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'tinygo') {
    Write-Host "Building the TinyGo test components" -ForegroundColor Green
    foreach ($subdir in $tinygo_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm" -Force -ErrorAction SilentlyContinue
            Remove-Item -Path "binding" -Recurse -Force -ErrorAction SilentlyContinue
        }
        & wit-bindgen-go generate --out binding --world $subdir ./wit

        $target = "../$subdir.wasm"
        & tinygo build -target=wasip2 -wit-package wit -wit-world $subdir -tags=purego -o $target main.go

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'grain') {
    Write-Host "Building the Grain test components" -ForegroundColor Green
    foreach ($subdir in $grain_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm" -Force -ErrorAction SilentlyContinue
        }
        & grain compile --release main.gr

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        & wasm-tools component new main.gr.wasm -o $target --adapt ../../../golem-wit/adapters/tier2/wasi_snapshot_preview1.wasm
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'js') {
    Write-Host "Building the JS test components" -ForegroundColor Green
    foreach ($subdir in $js_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm", "package-lock.json" -Force -ErrorAction SilentlyContinue
            Remove-Item -Path "node_modules" -Recurse -Force -ErrorAction SilentlyContinue
        }
        New-Item -Path "out" -ItemType Directory -Force | Out-Null
        & npm install
        & npm run build

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        Copy-Item -Path "out/component.wasm" -Destination $target -Verbose
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'java') {
    Write-Host "Building the Java test components" -ForegroundColor Green
    foreach ($subdir in $java_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            & mvn clean
        }
        & wit-bindgen teavm-java ./wit --out-dir src/main/java
        & mvn prepare-package

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        & wasm-tools component new target/generated/wasm/teavm-wasm/classes.wasm -o $target --adapt ../../../golem-wit/adapters/tier1/wasi_snapshot_preview1.wasm
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'dotnet') {
    Write-Host "Building the .NET test components" -ForegroundColor Green
    foreach ($subdir in $dotnet_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            & dotnet clean
        }
        & dotnet build -p:Configuration=Release

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        & wasm-tools component new "bin/Release/net7.0/$subdir.wasm" -o $target --adapt ../../../golem-wit/adapters/tier2/wasi_snapshot_preview1.wasm
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'swift') {
    Write-Host "Building the Swift test components" -ForegroundColor Green
    foreach ($subdir in $swift_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm" -Force -ErrorAction SilentlyContinue
        }
        # Note: The swiftc path is hardcoded for macOS and may require adjustment on other systems.
        & /Library/Developer/Toolchains/swift-latest.xctoolchain/usr/bin/swiftc -target wasm32-unknown-wasi main.swift -o main.wasm -sdk /Library/Developer/Toolchains/swift-latest.xctoolchain/usr/share/wasi-sysroot/
        & wasm-opt -Os main.wasm -o main.opt.wasm

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        & wasm-tools component new main.opt.wasm -o $target --adapt ../../../golem-wit/adapters/tier2/wasi_snapshot_preview1.wasm
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'c') {
    Write-Host "Building the C test components" -ForegroundColor Green
    foreach ($subdir in $c_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm" -Force -ErrorAction SilentlyContinue
        }
        & wit-bindgen c --autodrop-borrows yes ./wit
        # last built with wasi-sdk-0.25.0
        if (-not ($env:WASI_SDK_PATH) -or -not (Test-Path $env:WASI_SDK_PATH)) {
            throw "WASI_SDK_PATH environment variable is not set or points to an invalid path."
        }
        $clang = Join-Path $env:WASI_SDK_PATH "bin/clang"
        $sysroot = Join-Path $env:WASI_SDK_PATH "share/wasi-sysroot"
        & $clang --sysroot $sysroot main.c c_api1.c c_api1_component_type.o -o main.wasm

        Write-Host "Turning the module into a WebAssembly Component..."
        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        & wasm-tools component new main.wasm -o $target --adapt ../../../golem-wit/adapters/tier1/wasi_snapshot_preview1.wasm
        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'python') {
    Write-Host "Building the Python test components" -ForegroundColor Green
    foreach ($subdir in $python_test_components) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent -and (Test-Path "wit/deps.toml")) {
            & wit-deps update
        }

        if ($rebuild.IsPresent) {
            Remove-Item -Path "*.wasm" -Force -ErrorAction SilentlyContinue
            Remove-Item -Path "bindings" -Recurse -Force -ErrorAction SilentlyContinue
        }

        Write-Host "Compiling the python code into a WebAssembly Component..."
        Remove-Item -Path "bindings" -Recurse -Force -ErrorAction SilentlyContinue
        & componentize-py bindings bindings
        & componentize-py componentize test -o "${subdir}_full.wasm"
        & wasm-tools strip "${subdir}_full.wasm" -o "${subdir}.wasm"

        $target = "../$subdir.wasm"
        $target_wat = "../$subdir.wat"
        Move-Item -Path "$subdir.wasm" -Destination $target

        & wasm-tools print $target | Set-Content -Path $target_wat

        Pop-Location
    }
}

if (-not $single_lang -or $lang -eq 'ts') {
    Write-Host "Building the TS test apps" -ForegroundColor Green
    foreach ($subdir in $ts_test_apps) {
        Write-Host "Building $subdir..."
        Push-Location $subdir

        if ($updateWit.IsPresent) {
            & golem-cli app update-wit-deps
        }

        if ($rebuild.IsPresent) {
            & golem-cli app clean
        }

        & golem-cli app build
        & golem-cli app copy

        Pop-Location
    }
}