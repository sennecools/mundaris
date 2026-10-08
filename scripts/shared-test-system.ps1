param(
    [switch]$Build,
    [switch]$BuildOnly,
    [switch]$DeveloperInterface,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$sourceRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$workspaceRoot = 'D:\Mundaris'
$buildRoot = Join-Path $workspaceRoot 'target\prepared-profile-repo'
$receiptPath = Join-Path $buildRoot 'shared-test-build.json'
$binaryPath = Join-Path $buildRoot 'release\mundaris_app.exe'
function SourceManifest {
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($sourceRoot)
    $sourceFiles = [Collections.Generic.List[IO.FileInfo]]::new()
    while ($pending.Count -gt 0) {
        foreach ($entry in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
            if ($entry.Name -in @('.git', 'target')) { continue }
            if ($entry.PSIsContainer) { $pending.Push($entry.FullName) }
            else { $sourceFiles.Add($entry) }
        }
    }
    @($sourceFiles | Sort-Object FullName | ForEach-Object {
        [ordered]@{path=$_.FullName.Substring($sourceRoot.Length+1).Replace('\','/');
            bytes=$_.Length; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
    })
}
if ($BuildOnly -and -not $Build) { throw '-BuildOnly requires -Build' }
if (Get-Process -Name mundaris_app -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $binaryPath }) {
    throw 'Close this checkout native app before building or launching another run.'
}
if ($Build) {
    $before = SourceManifest
    Push-Location $sourceRoot
    try {
        & cargo build --locked --release -p mundaris_app --features developer-tools --bins --target-dir $buildRoot
        if ($LASTEXITCODE -ne 0) { throw "Build failed: $LASTEXITCODE" }
        $head = (& git rev-parse HEAD).Trim()
    } finally { Pop-Location }
    $after = SourceManifest
    if (($before | ConvertTo-Json -Depth 5 -Compress) -ne ($after | ConvertTo-Json -Depth 5 -Compress)) {
        throw 'Source changed during build; repeat with a stable checkout.'
    }
    $receipt = [ordered]@{schema_version=1; source_root=$sourceRoot; head=$head;
        built_utc=[DateTime]::UtcNow.ToString('o'); features=@('developer-tools');
        scene='mundaris.shared-test-system/1'; source_files=$after;
        binary_sha256=(Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()}
    $receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $receiptPath -Encoding utf8
}
if ($BuildOnly) { Write-Output $receiptPath; return }
if (-not (Test-Path -LiteralPath $receiptPath)) { throw 'Run with -Build first.' }
$saved = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
$current = SourceManifest
if (($current | ConvertTo-Json -Depth 5 -Compress) -ne ($saved.source_files | ConvertTo-Json -Depth 5 -Compress)) {
    throw 'Source or assets changed after build; run with -Build.'
}
if ((Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $saved.binary_sha256) {
    throw 'Binary differs from the build receipt.'
}
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $buildRoot ('native-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff'))
}
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) { throw 'OutputDirectory must be absolute.' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh output directory.' }
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$vars = @{
    MUNDARIS_TEST_SYSTEM=(Join-Path $sourceRoot 'assets\prepared\shared-test-system.json');
    MUNDARIS_DEV_OUTPUT=$OutputDirectory;
    MUNDARIS_DEV_REGISTRY=(Join-Path $OutputDirectory 'session-registry');
    MUNDARIS_DEV_BUILD_MANIFEST=$receiptPath;
    MUNDARIS_CAPTURE_DIR=(Join-Path $OutputDirectory 'captures')
}
$previous = @{}
try {
    foreach ($key in $vars.Keys) { $previous[$key]=[Environment]::GetEnvironmentVariable($key,'Process'); [Environment]::SetEnvironmentVariable($key,$vars[$key],'Process') }
    $arguments = @('--solar-system')
    if ($DeveloperInterface) { $arguments += '--dev-interface' }
    $process = Start-Process -FilePath $binaryPath -ArgumentList $arguments -WorkingDirectory $workspaceRoot -PassThru -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.log')
    [ordered]@{pid=$process.Id; output_directory=$OutputDirectory; binary=$binaryPath; receipt=$receiptPath} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $OutputDirectory 'launch.json')
} finally {
    foreach ($key in $vars.Keys) { [Environment]::SetEnvironmentVariable($key,$previous[$key],'Process') }
}
