#Requires -Version 5.1
<#
.SYNOPSIS
Run focused developer capture, interface tests, and formatting checks.
.DESCRIPTION
Each invocation writes isolated evidence under target/ai-check/<timestamp> unless
-OutputDirectory is supplied. The target must not already contain files.
#>
[CmdletBinding()]
param(
    [string] $OutputDirectory,
    [ValidateSet('solar-overview', 'earth-orbit', 'earth-close', 'moon-orbit')]
    [string] $Scene = 'earth-orbit'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')
    $OutputDirectory = Join-Path $repo ("target/ai-check/$stamp")
} elseif (-not [IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path (Get-Location).Path $OutputDirectory
}
$runDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $runDirectory) {
    if (-not (Test-Path -LiteralPath $runDirectory -PathType Container) -or
        @(Get-ChildItem -LiteralPath $runDirectory -Force).Count -gt 0) {
        throw "Output directory must be empty: $runDirectory"
    }
} else {
    $null = New-Item -ItemType Directory -Path $runDirectory -Force
}
$logsDirectory = Join-Path $runDirectory 'logs'
$null = New-Item -ItemType Directory -Path $logsDirectory
$testResultsPath = Join-Path $runDirectory 'test-results.txt'
$script:records = New-Object System.Collections.Generic.List[object]
$script:overallFailure = $false

function Invoke-CheckedNative {
    param([string] $Name, [string] $Executable, [string[]] $Arguments, [string] $LogPath)
    $started = [DateTime]::UtcNow
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $lines = @()
    $exitCode = 1
    $previousPreference = $ErrorActionPreference
    try {
        $native = (Get-Command $Executable -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        # Windows PowerShell wraps native stderr in ErrorRecords; warnings are not failures.
        $ErrorActionPreference = 'Continue'
        $lines = @(& $native @Arguments 2>&1 | ForEach-Object { $_.ToString() })
        $exitCode = $LASTEXITCODE
    } catch {
        $lines += $_.Exception.Message
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    $timer.Stop()
    [IO.File]::WriteAllLines($LogPath, [string[]]$lines, (New-Object Text.UTF8Encoding($false)))
    $passed = $exitCode -eq 0
    if (-not $passed) { $script:overallFailure = $true }
    $script:records.Add([pscustomobject]@{
        name = $Name; command = ($Executable + ' ' + ($Arguments -join ' '))
        started_utc = $started.ToString('o'); exit_code = $exitCode
        duration_seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 3)
        result = $(if ($passed) { 'pass' } else { 'fail' }); log = $LogPath
    })
    return $passed
}

$initialStatus = ''
$head = ''
$statusOk = $false
$headOk = $false
Push-Location $repo
try {
    $gitStarted = [DateTime]::UtcNow
    $gitTimer = [Diagnostics.Stopwatch]::StartNew()
    $initialStatus = (& git status --short 2>&1 | Out-String).TrimEnd()
    $statusExit = $LASTEXITCODE
    $statusOk = $statusExit -eq 0
    if (-not $statusOk) { $initialStatus = "ERROR: $initialStatus"; $script:overallFailure = $true }
    $gitTimer.Stop()
    $script:records.Add([pscustomobject]@{
        name = 'initial_git_status'; command = 'git status --short'; started_utc = $gitStarted.ToString('o')
        exit_code = $statusExit; duration_seconds = [Math]::Round($gitTimer.Elapsed.TotalSeconds, 3)
        result = $(if ($statusOk) { 'pass' } else { 'fail' }); log = ''
    })
    $gitStarted = [DateTime]::UtcNow
    $gitTimer.Restart()
    $head = (& git rev-parse HEAD 2>&1 | Out-String).Trim()
    $headExit = $LASTEXITCODE
    $headOk = $headExit -eq 0
    if (-not $headOk) { $head = "ERROR: $head"; $script:overallFailure = $true }
    $gitTimer.Stop()
    $script:records.Add([pscustomobject]@{
        name = 'initial_head'; command = 'git rev-parse HEAD'; started_utc = $gitStarted.ToString('o')
        exit_code = $headExit; duration_seconds = [Math]::Round($gitTimer.Elapsed.TotalSeconds, 3)
        result = $(if ($headOk) { 'pass' } else { 'fail' }); log = ''
    })

    [IO.File]::WriteAllText((Join-Path $runDirectory 'git-status.txt'), $initialStatus, (New-Object Text.UTF8Encoding($false)))
    $sourceFiles = @(& git ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml crates)
    if ($LASTEXITCODE -ne 0) { throw 'Source file discovery failed.' }
    $fingerprint = @($sourceFiles | Sort-Object -Unique | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | ForEach-Object {
        [pscustomobject]@{ path=$_; sha256=(Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }
    })
    [IO.File]::WriteAllText((Join-Path $runDirectory 'source-files.sha256.json'), (ConvertTo-Json -InputObject $fingerprint -Depth 3), [Text.UTF8Encoding]::new($false))
    $null = Invoke-CheckedNative 'format' 'cargo' @('fmt', '--all', '--', '--check') (Join-Path $logsDirectory 'format.log')
    $testLog = Join-Path $logsDirectory 'developer-interface.log'
    $null = Invoke-CheckedNative 'developer_interface' 'cargo' @('test', '--locked', '-p', 'mundaris_app', '--test', 'developer_interface') $testLog
    Copy-Item -LiteralPath $testLog -Destination $testResultsPath -Force
    $null = Invoke-CheckedNative 'developer_commands' 'cargo' @('test','--locked','-p','mundaris_app','--features','developer-tools','--test','developer_commands') (Join-Path $logsDirectory 'developer-commands.log')
    $null = Invoke-CheckedNative 'developer_bridge' 'cargo' @('test','--locked','-p','mundaris_app','--features','developer-tools','--test','developer_bridge') (Join-Path $logsDirectory 'developer-bridge.log')
    $null = Invoke-CheckedNative 'developer_capture' 'cargo' @(
        'run', '--locked', '--release', '-p', 'mundaris_app', '--features',
        'terrain-capture,surface-profile', '--example', 'developer_capture', '--',
        $Scene, $runDirectory
    ) (Join-Path $logsDirectory 'developer-capture.log')
    foreach ($artifact in @("$Scene.png", "$Scene.json")) {
        $path = Join-Path $runDirectory $artifact
        $ok = (Test-Path -LiteralPath $path -PathType Leaf) -and (Get-Item -LiteralPath $path).Length -gt 0
        $script:records.Add([pscustomobject]@{
            name = "artifact:$artifact"; command = "verify $path"; started_utc = [DateTime]::UtcNow.ToString('o')
            exit_code = $(if ($ok) { 0 } else { 1 }); duration_seconds = 0
            result = $(if ($ok) { 'pass' } else { 'fail' }); log = ''
        })
        if (-not $ok) { $script:overallFailure = $true }
    }
    $snapshotNote = 'Snapshot unavailable.'
    $snapshotTimer = [Diagnostics.Stopwatch]::StartNew()
    $snapshotOk = $false
    try {
        $snapshot = Get-Content -LiteralPath (Join-Path $runDirectory "$Scene.json") -Raw | ConvertFrom-Json
        if ($snapshot.schema_version -ne 6 -or $snapshot.capture.scene -ne $Scene -or $snapshot.capture.image -ne "$Scene.png") { throw 'Capture snapshot association mismatch.' }
        $expectedMode = switch ($Scene) { 'solar-overview' { 'system_orbit' } 'earth-close' { 'surface_inspection' } default { 'body_orbit' } }
        $expectedFocus = switch ($Scene) { 'solar-overview' { $null } 'moon-orbit' { 4 } default { 3 } }
        if ($snapshot.general.camera_mode -ne $expectedMode -or $snapshot.general.focused_body.index -ne $expectedFocus) { throw 'Capture camera/body fixture mismatch.' }
        $terrainName = if ($null -eq $snapshot.terrain.active_body) { 'not active' } else { $snapshot.terrain.active_body.name }
        $quality = if ($null -eq $snapshot.terrain.quality_pending) { 'n/a' } else { $snapshot.terrain.quality_pending }
        $settled = if ($null -eq $snapshot.terrain.settled) { 'n/a' } else { $snapshot.terrain.settled }
        $snapshotNote = "Terrain: $terrainName; quality_pending=$quality; settled=$settled; warnings=$($snapshot.warnings -join ', ')."
        $snapshotOk = $true
    } catch {
        $script:overallFailure = $true
        $snapshotNote = "Snapshot validation failed: $($_.Exception.Message)"
    }
    $snapshotTimer.Stop()
    $script:records.Add([pscustomobject]@{
        name='snapshot_association'; command="verify schema and scene in $Scene.json"; exit_code=$(if ($snapshotOk) { 0 } else { 1 })
        duration_seconds=$snapshotTimer.Elapsed.TotalSeconds; result=$(if ($snapshotOk) { 'pass' } else { 'fail' }); log=''
    })
} catch {
    $script:overallFailure = $true
    throw
} finally {
    Pop-Location
    $summary = @(
        '# AI check summary', '',
        "- Scene: $Scene", "- Git HEAD: $head", "- Initial status command succeeded: $statusOk",
        '- Initial Git state: `git-status.txt` (dirty source is not identified by HEAD alone).',
        "- Overall: $(if ($script:overallFailure) { 'FAIL' } else { 'PASS' })",
        "- $snapshotNote",
        "- Capture pair: [$Scene.png]($Scene.png) + [$Scene.json]($Scene.json)",
        '- Evidence limits: this is a fast developer check, not full validation. Inspect the image; tests/artifact presence alone do not prove visual correctness or performance improvement.',
        '- Command output is in `logs/`; targeted test output is additionally in `test-results.txt`.'
    )
    [IO.File]::WriteAllLines((Join-Path $runDirectory 'summary.md'), [string[]]$summary, (New-Object Text.UTF8Encoding($false)))
    $validation = [pscustomobject]@{
        scene = $Scene; output_directory = $runDirectory; head = $head; head_exit_ok = $headOk
        initial_git_status = $initialStatus; git_status_exit_ok = $statusOk
        commands = @($script:records.ToArray()); result = $(if ($script:overallFailure) { 'fail' } else { 'pass' })
    }
    [IO.File]::WriteAllText((Join-Path $runDirectory 'validation.json'), ($validation | ConvertTo-Json -Depth 5), [Text.UTF8Encoding]::new($false))
    Write-Output "AI check evidence: $runDirectory"
}
if ($script:overallFailure) { exit 1 }
