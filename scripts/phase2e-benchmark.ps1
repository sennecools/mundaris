#Requires -Version 5.1
<#
.SYNOPSIS
Run serial, matched native Phase 2E terrain evidence workloads.
.DESCRIPTION
Builds no binaries and changes no application files. Each backend/profile/repetition
trial owns its process, native evidence directory, and control lease. Raw runner
artifacts remain under the selected evidence root; summary.json contains compact
numeric comparisons and null for unavailable metrics.
#>
[CmdletBinding()]
param(
    [string] $ExecutablePath = '../target/release/mundaris_app.exe',
    [string] $DeveloperCliPath = '../target/release/mundaris_dev.exe',
    [string] $FrozenSourceDirectory = '',
    [string] $EvidenceRoot = '',
    [ValidateSet('Sweep','FarOrbit','100km','Close')][string] $Workload = 'Sweep',
    [ValidateSet('Resident','Legacy','Both')][string] $Backend = 'Both',
    [ValidateSet('Off','On','Both')][string] $Profile = 'Both',
    [ValidateRange(1,10)][int] $Repetitions = 1,
    [ValidateRange(1,60)][int] $DurationSeconds = 5,
    [ValidateRange(1,300)][int] $TimeoutSeconds = 90,
    [ValidateRange(0.1,5)][double] $PollSeconds = 0.5,
    [ValidateRange(0,7680)][int] $WindowWidth = 0,
    [ValidateRange(0,4320)][int] $WindowHeight = 0,
    [switch] $CaptureCheckpoints,
    [switch] $DisableAutomaticCaptures,
    [switch] $KeepBackground,
    [ValidateCount(1,6)][double[]] $CheckpointCaptureOffsetsSeconds = @(0,0.5,1,2,5,10)
)
$ErrorActionPreference = 'Stop'
if (($WindowWidth -eq 0) -ne ($WindowHeight -eq 0)) { throw 'Specify both window dimensions or neither.' }
if ($CheckpointCaptureOffsetsSeconds | Where-Object { $_ -lt 0 -or $_ -gt $TimeoutSeconds }) { throw 'Capture offsets must be nonnegative and no greater than TimeoutSeconds.' }
$repoRoot = Split-Path -Parent $PSScriptRoot
$runnerPath = Join-Path $PSScriptRoot 'planetary-resident-evidence.ps1'
$scriptPath = [IO.Path]::GetFullPath($PSCommandPath)
$scriptHash = (Get-FileHash -LiteralPath $scriptPath -Algorithm SHA256).Hash.ToLowerInvariant()
$runnerHash = (Get-FileHash -LiteralPath $runnerPath -Algorithm SHA256).Hash.ToLowerInvariant()
$resolvedFrozenSourceDirectory = ''
if (-not [string]::IsNullOrWhiteSpace($FrozenSourceDirectory)) {
    $resolvedFrozenSourceDirectory = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $FrozenSourceDirectory -ErrorAction Stop).ProviderPath)
    if (-not (Test-Path -LiteralPath (Join-Path $resolvedFrozenSourceDirectory 'Cargo.toml') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $resolvedFrozenSourceDirectory 'crates') -PathType Container)) {
        throw "Frozen source root must contain Cargo.toml and crates/: $resolvedFrozenSourceDirectory"
    }
}
if ([string]::IsNullOrWhiteSpace($EvidenceRoot)) {
    $EvidenceRoot = Join-Path $repoRoot (Join-Path '../target/terrain-redesign/phase2e' ([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '-' + [Guid]::NewGuid().ToString('N').Substring(0,8)))
}
$outputPath = if ([IO.Path]::IsPathRooted($EvidenceRoot)) { [IO.Path]::GetFullPath($EvidenceRoot) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $EvidenceRoot)) }
if (Test-Path -LiteralPath $outputPath) { throw "Evidence root already exists: $outputPath" }
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$fingerprint = [pscustomobject]@{
    head = ((& git -C $repoRoot rev-parse HEAD) -join '').Trim()
    git_status = @(& git -C $repoRoot status --short)
    executable = if ([IO.Path]::IsPathRooted($ExecutablePath)) { [IO.Path]::GetFullPath($ExecutablePath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $ExecutablePath)) }
    executable_sha256 = $null
    developer_cli = if ([IO.Path]::IsPathRooted($DeveloperCliPath)) { [IO.Path]::GetFullPath($DeveloperCliPath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $DeveloperCliPath)) }
    developer_cli_sha256 = $null
    frozen_source_directory = $resolvedFrozenSourceDirectory
    benchmark_script = $scriptPath; benchmark_script_sha256 = $scriptHash
    route_runner_script = [IO.Path]::GetFullPath($runnerPath); route_runner_script_sha256 = $runnerHash
}
foreach ($key in @('executable','developer_cli')) {
    if (Test-Path -LiteralPath $fingerprint.$key -PathType Leaf) { $fingerprint."${key}_sha256" = (Get-FileHash -LiteralPath $fingerprint.$key -Algorithm SHA256).Hash.ToLowerInvariant() }
}
function Write-JsonFile([string] $Path, $Value) {
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 40), $utf8)
}
function Get-ChildValue($Object, [string] $Path) {
    $current = $Object
    foreach ($part in $Path.Split('.')) {
        if ($null -eq $current) { return $null }
        $property = $current.PSObject.Properties[$part]
        if ($null -eq $property) { return $null }
        $current = $property.Value
    }
    return $current
}
function Get-CheckpointMetrics($Checkpoint) {
    [pscustomobject]@{
        useful_coverage_elapsed_ms = $Checkpoint.useful_coverage_elapsed_ms
        useful_detail_elapsed_ms = $Checkpoint.useful_detail_elapsed_ms
        settled_quality_elapsed_ms = $Checkpoint.settled_quality_elapsed_ms
        warm_interval_p50_ms = Get-ChildValue $Checkpoint 'frame_distributions.interval_ms.p50'
        warm_interval_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.interval_ms.p95'
        warm_frame_cpu_p50_ms = Get-ChildValue $Checkpoint 'frame_distributions.frame_cpu_ms.p50'
        warm_frame_cpu_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.frame_cpu_ms.p95'
        warm_terrain_preparation_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.terrain_preparation_ms.p95'
        warm_publication_combined_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.publication_ms.p95'
        warm_publication_admission_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.publication_admission_ms.p95'
        warm_gpu_terrain_p95_ms = Get-ChildValue $Checkpoint 'frame_distributions.gpu_terrain_ms.p95'
        warm_sampled_tile_content_upload_bytes = Get-ChildValue $Checkpoint 'warm_deltas.sampled_tile_content_upload_bytes'
        warm_sampled_boundary_upload_bytes = Get-ChildValue $Checkpoint 'warm_deltas.sampled_boundary_upload_bytes'
        warm_completed_generation_tiles = Get-ChildValue $Checkpoint 'warm_deltas.completed_generation_tiles'
        useful_detail_reached = $Checkpoint.useful_detail_reached
        settled_quality = $Checkpoint.settled_quality
        quality_timeout = $Checkpoint.quality_timeout
        sampled_frame_count = $Checkpoint.frame_sample_count
        requested_warm_seconds = $Checkpoint.warm_interval_seconds
        warm_sample_completion_ratio = Get-ChildValue $Checkpoint 'warm_sample_coverage.completion_ratio'
        warm_sample_expected_frames = Get-ChildValue $Checkpoint 'warm_sample_coverage.expected_frame_count'
        warm_sample_observed_frames = Get-ChildValue $Checkpoint 'warm_sample_coverage.observed_frame_count'
        warm_sample_missing_frames = Get-ChildValue $Checkpoint 'warm_sample_coverage.missing_frame_count'
    }
}
$availableBackends = if ($Backend -eq 'Both') { @('Resident','Legacy') } else { @($Backend) }
$availableProfiles = if ($Profile -eq 'Both') { @('Off','On') } else { @($Profile) }
$allTrials = @()
$runWatch = [Diagnostics.Stopwatch]::StartNew()
$runStartedUtc = [DateTime]::UtcNow.ToString('o')
$trialNumber = 0
Write-JsonFile (Join-Path $outputPath 'baseline.json') $fingerprint
foreach ($profileName in $availableProfiles) {
    foreach ($backendName in $availableBackends) {
        for ($repetition = 1; $repetition -le $Repetitions; $repetition++) {
            $trialNumber++
            $trialName = ('{0:D3}-{1}-{2}-r{3:D2}' -f $trialNumber,$Workload.ToLowerInvariant(),$profileName.ToLowerInvariant(),$repetition)
            if ($availableBackends.Count -gt 1) { $trialName = ('{0:D3}-{1}-{2}-{3}-r{4:D2}' -f $trialNumber,$Workload.ToLowerInvariant(),$backendName.ToLowerInvariant(),$profileName.ToLowerInvariant(),$repetition) }
            $trialDirectory = Join-Path $outputPath $trialName
            $trialWatch = [Diagnostics.Stopwatch]::StartNew()
            $trial = [ordered]@{
                trial_id=$trialName; status='running'; workload=$Workload; backend=$backendName; profile=$profileName
                repetition=$repetition; output_directory=$trialDirectory; requested_window_pixels=@($WindowWidth,$WindowHeight)
                requested_duration_seconds=$DurationSeconds; timeout_seconds=$TimeoutSeconds; poll_seconds=$PollSeconds
                route_runner_sha256=$runnerHash; started_utc=[DateTime]::UtcNow.ToString('o')
                elapsed_seconds=$null; source_sha256=$null; source_changed_during_run=$null; runner_script_sha256=$runnerHash; runner_script_changed_during_run=$null; executable_sha256=$fingerprint.executable_sha256
                checkpoint_metrics=@(); raw_summary=$null; failure=$null
            }
            $allTrials += [pscustomobject]$trial
            Write-JsonFile (Join-Path $outputPath 'trials-progress.json') $allTrials
            $oldProfile = $env:MUNDARIS_PROFILE
            $oldLab = $env:MUNDARIS_PERFORMANCE_LAB
            try {
                if ($profileName -eq 'On') { $env:MUNDARIS_PROFILE = '1'; $env:MUNDARIS_PERFORMANCE_LAB = '1' }
                else { Remove-Item Env:MUNDARIS_PROFILE -ErrorAction SilentlyContinue; Remove-Item Env:MUNDARIS_PERFORMANCE_LAB -ErrorAction SilentlyContinue }
                $childArgs = @('-NoLogo','-NoProfile','-ExecutionPolicy','Bypass','-File',$runnerPath,
                    '-ExecutablePath',$fingerprint.executable,'-DeveloperCliPath',$fingerprint.developer_cli,
                    '-EvidenceDirectory',$trialDirectory,'-RouteProfile',$Workload,
                    '-CheckpointTimeoutSeconds',[string]$TimeoutSeconds,'-WarmSeconds',[string]$DurationSeconds,'-PollSeconds',[string]$PollSeconds)
                if ($resolvedFrozenSourceDirectory) { $childArgs += @('-FrozenSourceDirectory',$resolvedFrozenSourceDirectory) }
                if ($backendName -eq 'Legacy') { $childArgs += '-LegacyTerrain' }
                if ($DisableAutomaticCaptures) { $childArgs += '-DisableAutomaticCaptures' }
                if ($KeepBackground) { $childArgs += '-KeepBackground' }
                if ($WindowWidth -gt 0) { $childArgs += @('-ClientWidth',[string]$WindowWidth,'-ClientHeight',[string]$WindowHeight) }
                if ($CaptureCheckpoints) {
                    $childArgs += '-CaptureCheckpoints'
                    $childArgs += '-CheckpointCaptureOffsetsSeconds'
                    $childArgs += @($CheckpointCaptureOffsetsSeconds | ForEach-Object { [string]$_ })
                }
                $shell = Get-Command powershell.exe -ErrorAction SilentlyContinue
                if ($null -eq $shell) { $shell = Get-Command pwsh.exe -ErrorAction Stop }
                & $shell.Source @childArgs
                $childExit = $LASTEXITCODE
                $trialWatch.Stop()
                if (Test-Path -LiteralPath (Join-Path $trialDirectory 'summary.json')) {
                    $raw = Get-Content -LiteralPath (Join-Path $trialDirectory 'summary.json') -Raw | ConvertFrom-Json
                    $trial.status = $raw.status
                    $trial.raw_summary = Join-Path $trialDirectory 'summary.json'
                    $trial.source_sha256 = $raw.fingerprints.source_sha256
                    $trial.source_changed_during_run = $raw.source_changed_during_run
                    $trial.runner_script_sha256 = $raw.fingerprints.runner_script_sha256
                    $trial.runner_script_changed_during_run = $raw.runner_script_changed_during_run
                    $trial.executable_sha256 = $raw.fingerprints.executable_sha256
                    $trial.checkpoint_metrics = @($raw.checkpoints | ForEach-Object { [pscustomobject]@{ name=$_.name; target_clearance_m=$_.target_clearance_m; metrics=(Get-CheckpointMetrics $_) } })
                    $trial.elapsed_seconds = [Math]::Round($trialWatch.Elapsed.TotalSeconds,3)
                    if ($childExit -ne 0 -and $raw.status -eq 'completed') { $trial.status = 'failed'; $trial.failure = "Route runner returned exit code $childExit despite completed summary." }
                } else {
                    $trial.status = 'failed'
                    $trial.failure = "Route runner exit code $childExit. See trial output and failure-artifacts under $trialDirectory."
                    $trial.elapsed_seconds = [Math]::Round($trialWatch.Elapsed.TotalSeconds,3)
                }
            } catch {
                $trialWatch.Stop()
                $trial.status = 'failed'
                $trial.failure = $_.Exception.Message
                $trial.elapsed_seconds = [Math]::Round($trialWatch.Elapsed.TotalSeconds,3)
            } finally {
                if ($null -eq $oldProfile) { Remove-Item Env:MUNDARIS_PROFILE -ErrorAction SilentlyContinue } else { $env:MUNDARIS_PROFILE = $oldProfile }
                if ($null -eq $oldLab) { Remove-Item Env:MUNDARIS_PERFORMANCE_LAB -ErrorAction SilentlyContinue } else { $env:MUNDARIS_PERFORMANCE_LAB = $oldLab }
            }
            $allTrials[-1] = [pscustomobject]$trial
            Write-JsonFile (Join-Path $outputPath 'trials-progress.json') $allTrials
        }
    }
}
$comparisons = @()
if ($Backend -eq 'Both') {
    foreach ($residentTrial in @($allTrials | Where-Object backend -EQ 'Resident')) {
        $legacyTrial = $allTrials | Where-Object { $_.backend -eq 'Legacy' -and $_.profile -eq $residentTrial.profile -and $_.repetition -eq $residentTrial.repetition } | Select-Object -First 1
        if ($null -eq $legacyTrial) { continue }
        foreach ($residentCheckpoint in $residentTrial.checkpoint_metrics) {
            $legacyCheckpoint = $legacyTrial.checkpoint_metrics | Where-Object name -EQ $residentCheckpoint.name | Select-Object -First 1
            if ($null -eq $legacyCheckpoint) { continue }
            $deltas = [ordered]@{}
            foreach ($property in $residentCheckpoint.metrics.PSObject.Properties) {
                if ($property.Value -is [ValueType] -and $property.Value -isnot [bool] -and $null -ne $property.Value) {
                    $legacyValue = $legacyCheckpoint.metrics.PSObject.Properties[$property.Name].Value
                    $deltas[$property.Name] = [pscustomobject]@{
                        legacy=$legacyValue; resident=$property.Value
                        resident_minus_legacy=$(if ($null -ne $legacyValue -and $legacyValue -is [ValueType]) { [double]$property.Value-[double]$legacyValue } else { $null })
                    }
                }
            }
            $comparisons += [pscustomobject]@{
                workload=$Workload; profile=$residentTrial.profile; repetition=$residentTrial.repetition
                checkpoint=$residentCheckpoint.name; target_clearance_m=$residentCheckpoint.target_clearance_m
                matched_build_identity=($residentTrial.executable_sha256 -eq $legacyTrial.executable_sha256 -and $residentTrial.source_sha256 -eq $legacyTrial.source_sha256 -and $residentTrial.runner_script_sha256 -eq $legacyTrial.runner_script_sha256 -and $residentTrial.source_changed_during_run -eq $false -and $legacyTrial.source_changed_during_run -eq $false -and $residentTrial.runner_script_changed_during_run -eq $false -and $legacyTrial.runner_script_changed_during_run -eq $false)
                resident_trial=$residentTrial.trial_id; legacy_trial=$legacyTrial.trial_id; metrics=$deltas
            }
        }
    }
}
$runWatch.Stop()
$failedTrials = @($allTrials | Where-Object status -NE 'completed')
$result = [pscustomobject]@{
    schema='mundaris.phase2e.benchmark.v1'
    status=$(if ($failedTrials.Count -eq 0) { 'completed' } else { 'partial' })
    started_utc=$runStartedUtc
    finished_utc=[DateTime]::UtcNow.ToString('o'); elapsed_seconds=[Math]::Round($runWatch.Elapsed.TotalSeconds,3)
    workload=$Workload; backend_policy=$Backend; profile_policy=$Profile; repetitions=$Repetitions
    requested_duration_seconds=$DurationSeconds; checkpoint_timeout_seconds=$TimeoutSeconds; poll_seconds=$PollSeconds
    requested_window_pixels=@($WindowWidth,$WindowHeight); checkpoint_capture_schedule_enabled=[bool]$CaptureCheckpoints
    checkpoint_capture_offsets_seconds=@($CheckpointCaptureOffsetsSeconds)
    fixed_route_configuration=[pscustomobject]@{ preset='solar-system'; body='Moon'; simulation_paused=$true; focus_body_fixed=$true; orientation='look-at Moon at launch; surface_inspection navigation'; normal_worker_delays=$true; trial_order='profile, backend, repetition; strictly serial' }
    fingerprints=$fingerprint
    trial_count=$allTrials.Count; failed_trial_count=$failedTrials.Count; trials=$allTrials; comparisons=$comparisons
    limitations=@('Native route and capture timings are wall-paced; scheduler, driver and capture calls add variable latency. Requested and actual offsets are recorded.', 'A null metric means the source did not expose it or the trial did not produce that sample; it is not zero.', 'This wrapper does not build binaries and does not prove visual acceptance from JSON or screenshots alone.')
}
Write-JsonFile (Join-Path $outputPath 'summary.json') $result
Write-JsonFile (Join-Path $outputPath 'trials.json') $allTrials
Write-Output "Phase 2E benchmark evidence: $outputPath"
if ($failedTrials.Count -gt 0) { exit 1 }
