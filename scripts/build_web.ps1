# Build the browser canvas host and generate matching JavaScript bindings.
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $projectRoot 'Cargo.toml'
$bindgen = Get-Command wasm-bindgen -ErrorAction Stop
$version = & $bindgen.Source --version
if ($LASTEXITCODE -ne 0 -or $version -ne 'wasm-bindgen 0.2.127') {
    throw 'This workspace needs wasm-bindgen-cli 0.2.127. Install it with: cargo install wasm-bindgen-cli --version 0.2.127 --locked'
}
& cargo build --locked --release --manifest-path $manifest -p countersplat-web --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { throw 'WebAssembly build failed.' }
$metadata = (& cargo metadata --no-deps --format-version 1 --manifest-path $manifest | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw 'Could not locate the Cargo target directory.' }
$wasm = Join-Path $metadata.target_directory 'wasm32-unknown-unknown/release/countersplat_web.wasm'
$output = Join-Path $projectRoot 'preview/pkg'
& $bindgen.Source --target web --out-dir $output --out-name countersplat_web $wasm
if ($LASTEXITCODE -ne 0) { throw 'Browser binding generation failed.' }
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/skybox-worker.js') -Destination (Join-Path $output 'skybox-worker.js') -Force
$luaOutput = Join-Path $output 'wasmoon'
New-Item -ItemType Directory -Path $luaOutput -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $projectRoot 'third_party/wasmoon/index.js') -Destination $luaOutput -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'third_party/wasmoon/glue.wasm') -Destination $luaOutput -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'crates/scripting-lua/src/engine_api.lua') -Destination (Join-Path $output 'engine_api.lua') -Force
# Package scene definitions and local image/script resources using their asset-relative paths.
$scenePath = Join-Path $projectRoot 'assets/scenes/gameplay.json'
$sceneDir = Split-Path -Parent $scenePath
$sceneOutput = Join-Path $output 'assets/scenes'
New-Item -ItemType Directory -Path $sceneOutput -Force | Out-Null
Copy-Item -LiteralPath $scenePath -Destination (Join-Path $sceneOutput 'gameplay.json') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/scenes/level-selector.json') -Destination (Join-Path $sceneOutput 'level-selector.json') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/scenes/menu.json') -Destination (Join-Path $sceneOutput 'menu.json') -Force
$scene = Get-Content -LiteralPath $scenePath -Raw | ConvertFrom-Json
$menu = Get-Content -LiteralPath (Join-Path $sceneDir 'menu.json') -Raw | ConvertFrom-Json
$references = @($scene.resources.assets.PSObject.Properties.Value) + @($menu.resources.assets.PSObject.Properties.Value) + @($scene.resources.scripts.PSObject.Properties.Value) + @($menu.resources.scripts.PSObject.Properties.Value)
$assetPrefix = [IO.Path]::GetFullPath((Join-Path $projectRoot 'assets')).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
$outputPrefix = [IO.Path]::GetFullPath($output).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
foreach ($reference in $references) {
    $uri = [string]$reference.uri
    if ($uri.StartsWith('/') -or $uri -match '^[a-zA-Z][a-zA-Z0-9+.-]*:') { continue }
    $source = [IO.Path]::GetFullPath((Join-Path $sceneDir $uri))
    $relative = [IO.Path]::GetRelativePath($projectRoot, $source)
    $destination = [IO.Path]::GetFullPath((Join-Path $output $relative))
    if (-not $source.StartsWith($assetPrefix, [StringComparison]::OrdinalIgnoreCase) -or
        -not $destination.StartsWith($outputPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Scene resource escapes the assets/package directory: $uri"
    }
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Scene resource is missing: $uri" }
    New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination -Force
}
Write-Host "CounterSplat canvas package generated in $output"
