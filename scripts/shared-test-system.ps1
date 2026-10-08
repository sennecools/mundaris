#Requires -Version 5.1
[CmdletBinding()]
param(
    [switch] $Build,
    [string] $OutputDirectory,
    [switch] $DeveloperInterface
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$contentRoot = [IO.Path]::GetFullPath((Join-Path $repo 'content'))
$executable = Join-Path $repo 'target\release\mundaris_app.exe'
$buildReceiptPath = Join-Path $repo 'target\shared-test-system-build.json'
$utf8 = New-Object Text.UTF8Encoding($false)

function Get-Sha256Bytes {
    param([byte[]] $Bytes)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Get-SourceSnapshot {
    Push-Location $repo
    try {
        $paths = @(& git ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml .cargo crates build.rs)
        if ($LASTEXITCODE -ne 0) { throw 'Could not enumerate locked build inputs with Git.' }
        $entries = @($paths | Sort-Object -Unique | ForEach-Object {
            $path = $_.Trim()
            if (-not $path) { return }
            $absolute = Join-Path $repo $path
            if (Test-Path -LiteralPath $absolute -PathType Leaf) {
                [pscustomobject]@{ path = $path.Replace('\', '/'); state = 'present'; sha256 = (Get-FileHash -LiteralPath $absolute -Algorithm SHA256).Hash.ToLowerInvariant() }
            } else {
                [pscustomobject]@{ path = $path.Replace('\', '/'); state = 'deleted'; sha256 = $null }
            }
        })
    } finally { Pop-Location }
    $json = ConvertTo-Json -InputObject $entries -Depth 4 -Compress
    [pscustomobject]@{ sha256 = Get-Sha256Bytes $utf8.GetBytes($json); files = $entries }
}

function Get-ContentSnapshot {
    param([string] $Root)
    $scenePath = Join-Path $Root 'test-solar-system.json'
    if (-not (Test-Path -LiteralPath $scenePath -PathType Leaf)) { throw "Canonical content is missing: $scenePath" }
    $scene = Get-Content -LiteralPath $scenePath -Raw | ConvertFrom-Json
    if ($scene.id -ne 'test-solar-system' -or [string]::IsNullOrWhiteSpace([string]$scene.camera_state)) {
        throw 'Content does not define the canonical test-solar-system and its camera state.'
    }
    $cameraPath = [IO.Path]::GetFullPath((Join-Path $Root ([string]$scene.camera_state)))
    $rootPrefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $cameraPath.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $cameraPath -PathType Leaf)) {
        throw 'Canonical camera path is missing or escapes the content directory.'
    }
    $entries = @(Get-ChildItem -LiteralPath $Root -Recurse -File | Sort-Object FullName | ForEach-Object {
        [pscustomobject]@{ path = $_.FullName.Substring($Root.Length).TrimStart('\').Replace('\', '/'); sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(); bytes = $_.Length }
    })
    $json = ConvertTo-Json -InputObject $entries -Depth 4 -Compress
    [pscustomobject]@{
        sha256 = Get-Sha256Bytes $utf8.GetBytes($json)
        scene = 'test-solar-system'
        scene_sha256 = (Get-FileHash -LiteralPath $scenePath -Algorithm SHA256).Hash.ToLowerInvariant()
        camera_path = $cameraPath
        camera_sha256 = (Get-FileHash -LiteralPath $cameraPath -Algorithm SHA256).Hash.ToLowerInvariant()
        files = $entries
    }
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $evidenceBase = 'D:\Mundaris\ai\tasks\2026-10-08-generation-optimization\evidence\shared-test-system'
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
    $OutputDirectory = Join-Path $evidenceBase ("$stamp-" + [Guid]::NewGuid().ToString('N'))
} elseif (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path (Get-Location).Path $OutputDirectory
}
$runDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $runDirectory) { throw "Output directory already exists; refusing to overwrite: $runDirectory" }
$null = New-Item -ItemType Directory -Path (Split-Path -Parent $runDirectory) -Force
$null = [IO.Directory]::CreateDirectory($runDirectory)
$claim = [IO.File]::Open((Join-Path $runDirectory '.shared-test-system-claim'), [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
$claim.Dispose()

$head = (& git -C $repo rev-parse HEAD 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw "Could not identify checkout HEAD: $head" }
$sourceBefore = Get-SourceSnapshot
$content = Get-ContentSnapshot $contentRoot

if ($Build) {
    Push-Location $repo
    try {
        & cargo build --locked --release -p mundaris_app --bin mundaris_app --bin mundaris_dev --features developer-tools --target-dir (Join-Path $repo 'target') *> (Join-Path $runDirectory 'build.log')
        if ($LASTEXITCODE -ne 0) { throw "Locked release developer-tools build failed; see $(Join-Path $runDirectory 'build.log')" }
    } finally { Pop-Location }
}
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Ordinary mundaris_app release executable is missing. Run this script with -Build: $executable"
}
$sourceAfter = Get-SourceSnapshot
if ($sourceBefore.sha256 -ne $sourceAfter.sha256) { throw 'Build inputs changed while preparing the launch; refusing uncertain attribution.' }
$contentAfter = Get-ContentSnapshot $contentRoot
if ($content.sha256 -ne $contentAfter.sha256) { throw 'Canonical content changed while preparing the launch; refusing uncertain attribution.' }
$executableHash = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Build) {
    $receipt = [pscustomobject]@{ source_sha256 = $sourceAfter.sha256; binary_sha256 = $executableHash; developer_binary_sha256 = (Get-FileHash -LiteralPath (Join-Path $repo 'target\release\mundaris_dev.exe') -Algorithm SHA256).Hash.ToLowerInvariant() }
    [IO.File]::WriteAllText($buildReceiptPath, (ConvertTo-Json $receipt), $utf8)
} else {
    if (-not (Test-Path -LiteralPath $buildReceiptPath -PathType Leaf)) { throw 'Missing shared build receipt. Run this launcher with -Build.' }
    $receipt = Get-Content -LiteralPath $buildReceiptPath -Raw | ConvertFrom-Json
    if ($receipt.source_sha256 -ne $sourceAfter.sha256 -or $receipt.binary_sha256 -ne $executableHash) { throw 'Executable build receipt does not match current source/binary. Run with -Build.' }
    if ($DeveloperInterface -and $receipt.developer_binary_sha256 -ne (Get-FileHash -LiteralPath (Join-Path $repo 'target\release\mundaris_dev.exe') -Algorithm SHA256).Hash.ToLowerInvariant()) { throw 'Developer executable differs from its shared build receipt.' }
}
$sourceManifest = [pscustomobject]@{ head = $head; sha256 = $sourceAfter.sha256; files = $sourceAfter.files }
$contentManifest = [pscustomobject]@{
    checkout_content_root = $contentRoot
    sha256 = $content.sha256
    scene = $content.scene
    scene_sha256 = $content.scene_sha256
    camera_path = $content.camera_path
    camera_sha256 = $content.camera_sha256
    files = $content.files
}
[IO.File]::WriteAllText((Join-Path $runDirectory 'source-manifest.json'), (ConvertTo-Json -InputObject $sourceManifest -Depth 6), $utf8)
[IO.File]::WriteAllText((Join-Path $runDirectory 'content-manifest.json'), (ConvertTo-Json -InputObject $contentManifest -Depth 6), $utf8)
$buildManifest = [pscustomobject]@{
    schema = 1
    executable = $executable
    binary_sha256 = $executableHash
    source_sha256 = $sourceAfter.sha256
    content_sha256 = $content.sha256
    scene = $content.scene
    scene_sha256 = $content.scene_sha256
    camera_path = $content.camera_path
    camera_sha256 = $content.camera_sha256
}
$buildManifestPath = Join-Path $runDirectory 'build-manifest.json'
[IO.File]::WriteAllText($buildManifestPath, (ConvertTo-Json -InputObject $buildManifest -Depth 4), $utf8)

$stdoutPath = Join-Path $runDirectory 'stdout.txt'
$stderrPath = Join-Path $runDirectory 'stderr.txt'
$registryPath = Join-Path $runDirectory 'registry'
$nativeOutputPath = Join-Path $runDirectory 'native-output'
$arguments = @()
if ($DeveloperInterface) {
    $arguments += '--dev-interface'
    $null = New-Item -ItemType Directory -Path $registryPath
    $null = New-Item -ItemType Directory -Path $nativeOutputPath
}
$environmentNames = @('MUNDARIS_DEV_REGISTRY', 'MUNDARIS_DEV_OUTPUT', 'MUNDARIS_DEV_BUILD_MANIFEST', 'MUNDARIS_PERFORMANCE_LAB', 'MUNDARIS_CAPTURE_DIR')
$previousEnvironment = @{}
foreach ($name in $environmentNames) { $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    if ($DeveloperInterface) {
        $env:MUNDARIS_DEV_REGISTRY = $registryPath
        $env:MUNDARIS_DEV_OUTPUT = $nativeOutputPath
        $env:MUNDARIS_DEV_BUILD_MANIFEST = $buildManifestPath
    }
    Remove-Item Env:MUNDARIS_PERFORMANCE_LAB, Env:MUNDARIS_CAPTURE_DIR -ErrorAction SilentlyContinue
    if (-not $DeveloperInterface) { Remove-Item Env:MUNDARIS_DEV_REGISTRY, Env:MUNDARIS_DEV_OUTPUT, Env:MUNDARIS_DEV_BUILD_MANIFEST -ErrorAction SilentlyContinue }
    $process = Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $repo -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
} finally {
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process') }
}

$launch = [pscustomobject]@{
    schema = 1
    started_utc = [DateTime]::UtcNow.ToString('o')
    pid = $process.Id
    process_name = 'mundaris_app'
    arguments = $arguments
    developer_interface = [bool]$DeveloperInterface
    checkout = $repo
    head = $head
    executable = $executable
    executable_sha256 = $executableHash
    source_sha256 = $sourceAfter.sha256
    content_root = $contentRoot
    content_sha256 = $content.sha256
    scene = $content.scene
    scene_sha256 = $content.scene_sha256
    camera_path = $content.camera_path
    camera_sha256 = $content.camera_sha256
    output_directory = $runDirectory
}
[IO.File]::WriteAllText((Join-Path $runDirectory 'launch.json'), (ConvertTo-Json -InputObject $launch -Depth 5), $utf8)

if ($DeveloperInterface) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    $descriptorPath = $null
    do {
        $descriptorPath = Get-ChildItem -LiteralPath $registryPath -File -Filter '*.json' -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
        if ($descriptorPath) { break }
        $running = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
        if (-not $running) { throw "Native process $($process.Id) exited before publishing its developer session. See $stderrPath" }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $descriptorPath) { throw "Native process $($process.Id) did not publish a developer session within 30 seconds." }
    $descriptor = Get-Content -LiteralPath $descriptorPath -Raw | ConvertFrom-Json
    if ([int]$descriptor.pid -ne $process.Id -or $descriptor.binary_sha256 -ne $executableHash) {
        throw 'Developer session descriptor does not match the launched PID and executable hash.'
    }
    Copy-Item -LiteralPath $descriptorPath -Destination (Join-Path $runDirectory 'session.json')
}

Write-Output "Shared test system launched: PID $($process.Id)"
Write-Output "Evidence: $runDirectory"
