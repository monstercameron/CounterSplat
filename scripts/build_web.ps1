# Build the browser canvas host and generate matching JavaScript bindings.
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $projectRoot 'Cargo.toml'
$bindgen = Get-Command wasm-bindgen -ErrorAction Stop
$version = & $bindgen.Source --version
if ($LASTEXITCODE -ne 0 -or $version -ne 'wasm-bindgen 0.2.127') {
    throw 'This workspace needs wasm-bindgen-cli 0.2.127. Install it with: cargo install wasm-bindgen-cli --version 0.2.127 --locked'
}
$cargoTarget = Join-Path $projectRoot '.artifacts/target'
& cargo build --locked --release --manifest-path $manifest --target-dir $cargoTarget -p countersplat-web --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { throw 'WebAssembly build failed.' }
$wasm = Join-Path $cargoTarget 'wasm32-unknown-unknown/release/countersplat_web.wasm'
$output = Join-Path $projectRoot '.artifacts/web/pkg'
& $bindgen.Source --target web --out-dir $output --out-name countersplat_web $wasm
if ($LASTEXITCODE -ne 0) { throw 'Browser binding generation failed.' }
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/skybox-worker.js') -Destination (Join-Path $output 'skybox-worker.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/voxel-worker.js') -Destination (Join-Path $output 'voxel-worker.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/voxel_runtime.js') -Destination (Join-Path $output 'voxel_runtime.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/mouse_capture.js') -Destination (Join-Path $output 'mouse_capture.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/mouse_buttons.js') -Destination (Join-Path $output 'mouse_buttons.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/first_person_frames.js') -Destination (Join-Path $output 'first_person_frames.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/display_settings.js') -Destination (Join-Path $output 'display_settings.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/frame_profile.js') -Destination (Join-Path $output 'frame_profile.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/player_rig.js') -Destination (Join-Path $output 'player_rig.js') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'preview/chunk_decoder_pool.js') -Destination (Join-Path $output 'chunk_decoder_pool.js') -Force
foreach ($module in @('chunk_preload.js', 'metadata_client.js', 'metadata_loader.js', 'metadata-worker.js', 'asset_loader.js')) {
    Copy-Item -LiteralPath (Join-Path $projectRoot "preview/$module") -Destination (Join-Path $output $module) -Force
}
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
$createdDirectories = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
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
    $destinationDirectory = [IO.Path]::GetDirectoryName($destination)
    if ($createdDirectories.Add($destinationDirectory)) {
        [IO.Directory]::CreateDirectory($destinationDirectory) | Out-Null
    }
    [IO.File]::Copy($source, $destination, $true)
}
Write-Host "CounterSplat canvas package generated in $output"
