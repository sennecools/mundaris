#Requires -Version 5.1
<#
.SYNOPSIS
Record a reproducible native Moon route for Slice 2D planetary terrain.
.DESCRIPTION
Starts the supplied application executable in an isolated developer-session
registry, records a real Solar System Moon route, and preserves source/binary
fingerprints, native PNG/JSON capture pairs, convergence observations, and timing
samples. The script owns only the process it starts. It never rebuilds the app.
#>
[CmdletBinding()]
param(
    [string] $ExecutablePath = '../target/release/mundaris_app.exe',
    [string] $DeveloperCliPath = '../target/release/mundaris_dev.exe',
    [string] $EvidenceDirectory = '',
    [string] $RegistryPath = '',
    [string] $SessionId = '',
    [switch] $LegacyTerrain,
    [switch] $DisableAutomaticCaptures,
    [ValidateRange(1,300)][int] $CheckpointTimeoutSeconds = 90,
    [ValidateRange(1,60)][int] $WarmSeconds = 5,
    [ValidateRange(0.1,5)][double] $PollSeconds = 0.5,
    [ValidateRange(1,180)][int] $StartupTimeoutSeconds = 45,
    [ValidateSet('Sweep','FarOrbit','100km','Close')][string] $RouteProfile = 'Sweep',
    [switch] $CaptureCheckpoints,
    [ValidateCount(1,6)][double[]] $CheckpointCaptureOffsetsSeconds = @(0,0.5,1,2,5,10),
    [ValidateRange(0,10)][int] $LimitCheckpoints = 0,
    [ValidateRange(0,9)][int] $SkipCheckpoints = 0,
    [ValidateRange(0,7680)][int] $ClientWidth = 0,
    [ValidateRange(0,4320)][int] $ClientHeight = 0,
    [switch] $KeepBackground,
    [string] $FrozenSourceDirectory = '',
    [switch] $FingerprintOnly
)
$ErrorActionPreference = 'Stop'
$frozenSourceDirectoryArgument = $FrozenSourceDirectory
if (($ClientWidth -eq 0) -ne ($ClientHeight -eq 0)) { throw 'Specify both client dimensions or neither.' }
if ($DisableAutomaticCaptures -and -not [string]::IsNullOrWhiteSpace($SessionId)) { throw '-DisableAutomaticCaptures applies only to a runner-owned child process.' }
$repoRoot = Split-Path -Parent $PSScriptRoot
$script:RunnerScriptPath = [IO.Path]::GetFullPath($PSCommandPath)
Push-Location $repoRoot
$script:FrozenSourceDirectory = ''
if (-not [string]::IsNullOrWhiteSpace($frozenSourceDirectoryArgument)) {
    $resolvedFrozenRoot = Resolve-Path -LiteralPath $frozenSourceDirectoryArgument -ErrorAction Stop
    $script:FrozenSourceDirectory = [IO.Path]::GetFullPath($resolvedFrozenRoot.ProviderPath)
    if (-not (Test-Path -LiteralPath (Join-Path $script:FrozenSourceDirectory 'Cargo.toml') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $script:FrozenSourceDirectory 'crates') -PathType Container)) {
        throw "Frozen source root must contain Cargo.toml and crates/: $script:FrozenSourceDirectory"
    }
}
$nativeProcess = $null
$lease = $null
$session = $null
$script:sessionWatch = [Diagnostics.Stopwatch]::StartNew()
$summary = [ordered]@{
    schema = 1
    evidence_metrics_schema = 3
    export_scope = [pscustomobject]@{
        poll_resident = 'compact scalar/configuration/resource/convergence/publication fields; high-volume histories and slot arrays omitted'
        poll_terrain_trace = 'schema, frame/time, tracked count, queues and block reasons; jobs and events omitted'
        progress = 'compact heartbeat projection'
        native_captures = 'full root JSON snapshot and paired images'
        final_summary = 'full selected evidence and deduplicated frame series'
    }
    status = 'running'
    started_utc = [DateTime]::UtcNow.ToString('o')
    started_monotonic_label = 'PowerShell runner Stopwatch origin; session_elapsed_seconds is monotonic within this run'
    route = 'solar-system Moon; normal native worker delays'
    route_profile = $RouteProfile
    checkpoint_capture_schedule_enabled = [bool]$CaptureCheckpoints
    checkpoint_capture_offsets_seconds = @($CheckpointCaptureOffsetsSeconds)
    checkpoint_limit = $LimitCheckpoints
    checkpoint_skip = $SkipCheckpoints
    requested_client_pixels = @($ClientWidth, $ClientHeight)
    keep_background = [bool]$KeepBackground
    automatic_captures_enabled = -not [bool]$DisableAutomaticCaptures
    legacy_terrain = [bool]$LegacyTerrain
    profile_requested = $env:MUNDARIS_PROFILE -eq '1'
    fingerprints = $null
    session = $null
    checkpoints = @()
    route_actions = @()
    rapid_action_frames = @()
    rapid_action_distributions = $null
    route_motion_frames = @()
    route_motion_distributions = $null
    read_only_transport_retries = @()
    bootstrap_frames = @()
    bootstrap_frame_distributions = $null
    failures = @()
}
function Write-JsonFile([string] $Path, $Value) {
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 40), $utf8)
}
function Write-ProgressFile {
    $completed = @($script:summary.checkpoints | ForEach-Object {
        [pscustomobject]@{
            name=$_.name
            route_phase=$_.route_phase
            target_clearance_m=$_.target_clearance_m
            session_end_elapsed_seconds=$_.session_end_elapsed_seconds
            camera_transition_stopped=$_.camera_transition_stopped
            useful_coverage_observed=$_.useful_coverage_observed
            useful_detail_reached=$_.useful_detail_reached
            settled_quality=$_.settled_quality
            failure=$_.failure
        }
    })
    $active = $null
    if ($null -ne $script:activeStageSeries) {
        $active = [pscustomobject]@{
            name=$script:activeStageSeries.name
            route_phase=$script:activeStageSeries.route_phase
            target_clearance_m=$script:activeStageSeries.target_clearance_m
            completed=$script:activeStageSeries.completed
            convergence_sample_count=@($script:activeStageSeries.convergence_samples).Count
            warm_sample_count=@($script:activeStageSeries.warm_samples).Count
            before_frame=$script:activeStageSeries.before_observation.frame_number
        }
    }
    $latestFailure = $null
    if (@($script:summary.failures).Count -gt 0) {
        $latestFailure = [string]$script:summary.failures[-1].message
        if ($latestFailure.Length -gt 1000) { $latestFailure = $latestFailure.Substring(0,1000) }
    }
    $progress = [pscustomobject]@{
        schema=$script:summary.schema
        evidence_metrics_schema=$script:summary.evidence_metrics_schema
        export_scope=$script:summary.export_scope
        status=$script:summary.status
        started_utc=$script:summary.started_utc
        updated_utc=[DateTime]::UtcNow.ToString('o')
        route_profile=$script:summary.route_profile
        checkpoint_capture_schedule_enabled=$script:summary.checkpoint_capture_schedule_enabled
        checkpoint_count=@($script:summary.checkpoints).Count
        completed_checkpoints=$completed
        active_checkpoint=$active
        route_action_count=@($script:summary.route_actions).Count
        failure_count=@($script:summary.failures).Count
        latest_failure=$latestFailure
    }
    Write-JsonFile (Join-Path $script:outputPath 'progress.json') $progress
}
function Get-HashOrNull([string] $Path) {
    if (Test-Path -LiteralPath $Path -PathType Leaf) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
    return $null
}
function Get-FrozenSourcePaths {
    $root = $script:FrozenSourceDirectory
    if ([string]::IsNullOrWhiteSpace($root)) { throw 'Frozen source root is not configured.' }
    $paths = @()
    $gitRootText = & git -C $root rev-parse --show-toplevel 2>$null
    $isGitRoot = $LASTEXITCODE -eq 0 -and [IO.Path]::GetFullPath(($gitRootText -join '').Trim()).TrimEnd([char[]]@('\','/')) -eq $root.TrimEnd([char[]]@('\','/'))
    if ($isGitRoot) {
        $gitPaths = @(& git -C $root ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml crates)
        if ($LASTEXITCODE -ne 0) { throw "Could not enumerate frozen source files under $root." }
        $paths = @($gitPaths | ForEach-Object { ([string]$_).Replace('\','/') } | Sort-Object -Unique)
    } else {
        foreach ($name in @('Cargo.toml','Cargo.lock','rust-toolchain.toml')) {
            if (Test-Path -LiteralPath (Join-Path $root $name) -PathType Leaf) { $paths += $name }
        }
        $pendingDirectories = [System.Collections.Generic.Stack[string]]::new()
        $pendingDirectories.Push((Join-Path $root 'crates'))
        $rootPrefix = $root.TrimEnd([char[]]@('\','/')) + [IO.Path]::DirectorySeparatorChar
        while ($pendingDirectories.Count -gt 0) {
            $directory = $pendingDirectories.Pop()
            foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force)) {
                if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
                if ($item.PSIsContainer) { $pendingDirectories.Push($item.FullName); continue }
                $fullPath = [IO.Path]::GetFullPath($item.FullName)
                if (-not $fullPath.StartsWith($rootPrefix,[StringComparison]::OrdinalIgnoreCase)) { throw "Frozen source path escaped its root: $fullPath" }
                $paths += $fullPath.Substring($rootPrefix.Length).Replace('\','/')
            }
        }
        $paths = @($paths | Sort-Object -Unique)
    }
    if ($paths.Count -eq 0 -or $paths -notcontains 'Cargo.toml') { throw "Frozen source root contains no eligible source manifest: $root" }
    return $paths
}
function Get-Fingerprints {
    $head = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Could not read baseline revision.' }
    $status = @(& git status --short)
    if ($LASTEXITCODE -ne 0) { throw 'Could not read repository status.' }
    if ($script:FrozenSourceDirectory) {
        if ($null -eq $script:frozenSourcePaths) { $script:frozenSourcePaths = @(Get-FrozenSourcePaths) }
        $paths = @($script:frozenSourcePaths)
    } else {
        $paths = @(& git ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml crates)
        if ($LASTEXITCODE -ne 0) { throw 'Could not enumerate source files.' }
    }
    $files = @($paths | Sort-Object -Unique | ForEach-Object {
        $relativePath = ([string]$_).Replace('/',[IO.Path]::DirectorySeparatorChar)
        $sourcePath = if ($script:FrozenSourceDirectory) { Join-Path $script:FrozenSourceDirectory $relativePath } else { $_ }
        if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) { throw "Enumerated source file disappeared: $sourcePath" }
        [pscustomobject]@{ path=([string]$_).Replace('\','/'); sha256=(Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
    $canonical = (($files | ForEach-Object { "$($_.path)=$($_.sha256)" }) -join "`n")
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $sourceSha = ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($canonical)))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
    [pscustomobject]@{
        head = $head
        git_status = $status
        source_file_count = $files.Count
        source_sha256 = $sourceSha
        source_origin = $(if ($script:FrozenSourceDirectory) { [IO.Path]::GetFullPath($script:FrozenSourceDirectory) } else { 'current working tree' })
        source_files = $files
        executable = [IO.Path]::GetFullPath($script:ExecutablePath)
        executable_sha256 = Get-HashOrNull $script:ExecutablePath
        developer_cli = [IO.Path]::GetFullPath($script:DeveloperCliPath)
        developer_cli_sha256 = Get-HashOrNull $script:DeveloperCliPath
        runner_script = $script:RunnerScriptPath
        runner_script_sha256 = Get-HashOrNull $script:RunnerScriptPath
    }
}
function Invoke-Dev([string[]] $Arguments) {
    $readOnly = $Arguments.Count -gt 0 -and $Arguments[0] -in @('inspect','diagnostics','receipt','events')
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        $started = [DateTime]::UtcNow
        $text = & $script:DeveloperCliPath --registry $script:registryPath --session $script:sessionId @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        if ($exitCode -eq 0) {
            try { return (($text -join "`n") | ConvertFrom-Json) }
            catch { throw "mundaris_dev returned invalid JSON for '$($Arguments -join ' ')': $text" }
        }
        $message = "mundaris_dev $($Arguments -join ' ') failed ($exitCode): $text"
        if (!$readOnly -or $attempt -eq 3 -or $message -notmatch 'os error (10053|10054|10060)|connection.*(reset|forcibly closed)') { throw $message }
        $script:summary.read_only_transport_retries += [pscustomobject]@{
            started_utc=$started.ToString('o'); operation=$Arguments[0]; failed_attempt=$attempt; message=$message
            policy='at most three attempts for read-only network resets; mutations and invalid JSON are never retried'
        }
        Start-Sleep -Milliseconds (100 * $attempt)
    }
}
function Get-Snapshot($Response) {
    if ($null -ne $Response.data.snapshot) { return $Response.data.snapshot }
    if ($null -ne $Response.data.data.snapshot) { return $Response.data.data.snapshot }
    throw 'inspect response did not contain a snapshot.'
}
function Renew-Control {
    $response = Invoke-Dev @('control','renew',$script:lease)
    if ($response.status -ne 'ok') {
        Record-ControlLoss "control renew returned '$($response.status)'"
        throw "Control renewal failed: $($response | ConvertTo-Json -Compress -Depth 8). See control-loss-events.json."
    }
}
function Record-ControlLoss([string] $Cause) {
    try {
        $events = Invoke-Dev @('events','0')
        if ($null -ne $script:outputPath -and (Test-Path -LiteralPath $script:outputPath)) {
            Write-JsonFile (Join-Path $script:outputPath 'control-loss-events.json') $events
        }
        $script:summary.control_loss_observation = [pscustomobject]@{
            cause=$Cause
            events_status=$events.status
            event_count=@($events.data.events).Count
            path=$(if ($null -ne $script:outputPath) { Join-Path $script:outputPath 'control-loss-events.json' } else { $null })
        }
    } catch {
        $script:summary.control_loss_observation = [pscustomobject]@{
            cause=$Cause
            event_collection_error=$_.Exception.Message
        }
    }
}
function Get-CompactTerrainTrace($Trace) {
    if ($null -eq $Trace) { return $null }
    $compact = [ordered]@{}
    foreach ($name in @('schema_version','snapshot_frame_id','generated_at_us','tracked_jobs','queues','block_reasons')) {
        $property = $Trace.PSObject.Properties[$name]
        if ($null -ne $property) { $compact[$name] = $property.Value }
    }
    return [pscustomobject]$compact
}
function Get-CompactResidentValue($Value) {
    if ($null -eq $Value) { return $null }
    if ($Value -is [System.Array]) {
        $items = [System.Collections.Generic.List[object]]::new()
        foreach ($item in $Value) { $items.Add((Get-CompactResidentValue $item)) }
        $compactItems = $items.ToArray()
        return ,$compactItems
    }
    if ($Value -isnot [pscustomobject]) { return $Value }
    $excluded = @('native_frame_samples','frame_intervals_ms','events','gpu_slots','desired','resident','drawable')
    $compact = [ordered]@{}
    foreach ($property in $Value.PSObject.Properties) {
        $name = [string]$property.Name
        if ($excluded -contains $name -or $name -match '^raw_?(desired|resident|drawable)(_|$)') { continue }
        if ($name -eq 'terrain_trace') {
            $compact[$name] = Get-CompactTerrainTrace $property.Value
        } else {
            $compact[$name] = Get-CompactResidentValue $property.Value
        }
    }
    return [pscustomobject]$compact
}
function Get-CompactResident($Resident) {
    return Get-CompactResidentValue $Resident
}
function Get-Resident($Snapshot, $Diagnostics) {
    if ($null -ne $Snapshot.resident_planetary) { return $Snapshot.resident_planetary }
    if ($null -ne $Diagnostics.data.resident_planetary) { return $Diagnostics.data.resident_planetary }
    if ($null -ne $Diagnostics.data.terrain.resident_planetary) { return $Diagnostics.data.terrain.resident_planetary }
    if ($null -ne $Diagnostics.data.performance.resident_planetary) { return $Diagnostics.data.performance.resident_planetary }
    return $null
}
function Get-Observation {
    $inspect = Invoke-Dev @('inspect')
    if ($inspect.status -ne 'ok') { throw "inspect returned status '$($inspect.status)'" }
    $snapshot = Get-Snapshot $inspect
    $diag = $null
    $resident = $snapshot.resident_planetary
    if ($null -eq $resident) {
        $diag = Invoke-Dev @('diagnostics','performance')
        $resident = Get-Resident $snapshot $diag
    }
    [pscustomobject]@{ snapshot=$snapshot; resident=$resident; diagnostics=$diag }
}
function Send-Action($Command, [string] $Name) {
    Renew-Control
    $actionStartElapsed = $script:sessionWatch.Elapsed.TotalSeconds
    $json = ConvertTo-Json -InputObject $Command -Compress -Depth 12
    $accepted = Invoke-Dev @('action',$script:lease,$json)
    if ($accepted.status -ne 'accepted' -and $accepted.status -ne 'ok') {
        Record-ControlLoss "action '$Name' rejected with status '$($accepted.status)'"
        throw "Action '$Name' rejected: $($accepted | ConvertTo-Json -Compress -Depth 8)"
    }
    $commandId = [string]$accepted.data.command_id
    if ([string]::IsNullOrWhiteSpace($commandId)) { throw "Action '$Name' did not return a command id." }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ($timer.Elapsed.TotalSeconds -lt 25) {
        $receiptResponse = Invoke-Dev @('receipt',$commandId)
        $receipt = $receiptResponse.data
        $status = if ($null -ne $receipt.status) { [string]$receipt.status } else { [string]$receiptResponse.status }
        if ($status -in @('applied','completed')) {
            $actionEndElapsed = $script:sessionWatch.Elapsed.TotalSeconds
            $script:summary.route_actions += [pscustomobject]@{ name=$Name; command=$Command; status=$status; command_id=$commandId; sequence=$receipt.sequence; session_start_elapsed_seconds=[Math]::Round($actionStartElapsed,3); session_end_elapsed_seconds=[Math]::Round($actionEndElapsed,3); action_receipt_latency_ms=[Math]::Round(($actionEndElapsed-$actionStartElapsed)*1000.0,3) }
            return
        }
        if ($status -in @('failed','cancelled','expired','not_found')) { throw "Action '$Name' ended with receipt status '$status'." }
        if ($timer.Elapsed.TotalSeconds -gt 10) { Renew-Control }
        Start-Sleep -Milliseconds 250
    }
    throw "Action '$Name' receipt timed out after 25 seconds."
}
function Get-Number($Object, [string[]] $Names) {
    foreach ($name in $Names) {
        $value = $Object.$name
        if ($null -ne $value -and $value -is [ValueType]) { return [double]$value }
    }
    return $null
}
function Get-FrameSamples($Resident) {
    if ($null -eq $Resident) { return @() }
    if ($null -ne $Resident.native_frame_samples) { return @($Resident.native_frame_samples) }
    if ($null -ne $Resident.regional.native_frame_samples) { return @($Resident.regional.native_frame_samples) }
    return @()
}
function Get-Pipeline($Resident) {
    if ($null -eq $Resident) { return $null }
    return $Resident.publication_pipeline
}
function Get-PipelineSnapshot($Resident) {
    $pipeline = Get-Pipeline $Resident
    if ($null -eq $pipeline) { return $null }
    [pscustomobject]@{
        generation_queued = Get-Number $pipeline @('generation_queued')
        generation_active = Get-Number $pipeline @('generation_active')
        generated_awaiting_boundary_preparation = Get-Number $pipeline @('generated_awaiting_boundary_preparation')
        boundary_preparation_queued_batches = Get-Number $pipeline @('boundary_preparation_queued_batches')
        boundary_preparation_active_batches = Get-Number $pipeline @('boundary_preparation_active_batches')
        boundary_preparation_completed_batches_awaiting_drain = Get-Number $pipeline @('boundary_preparation_completed_batches_awaiting_drain')
        fully_prepared_results = Get-Number $pipeline @('fully_prepared_results')
        immediately_publishable_results = Get-Number $pipeline @('immediately_publishable_results')
        blocked_by_split = Get-Number $pipeline @('blocked_by_split')
        blocked_by_merge = Get-Number $pipeline @('blocked_by_merge')
        blocked_by_neighbor_dependency = Get-Number $pipeline @('blocked_by_neighbor_dependency')
        blocked_by_residency_slot = Get-Number $pipeline @('blocked_by_residency_slot')
        publication_backlog_age_ms = Get-Number $pipeline @('publication_backlog_age_ms')
        maximum_publication_backlog_age_ms = Get-Number $pipeline @('maximum_publication_backlog_age_ms')
        boundary_preparation_completed_groups = Get-Number $pipeline @('boundary_preparation_completed_groups')
        publication_adopted_groups = Get-Number $pipeline @('publication_adopted_groups')
    }
}
function Get-PipelineSummary($Polls, $FrameSamples, [double] $ElapsedSeconds) {
    $diagnosticPolls = @($Polls | Where-Object { $null -ne $_.pipeline })
    $blockedFields = @('blocked_by_split','blocked_by_merge','blocked_by_neighbor_dependency','blocked_by_residency_slot')
    $blocked = [ordered]@{}
    foreach ($field in $blockedFields) {
        $values = @($diagnosticPolls | ForEach-Object { $value=$_.pipeline.$field; if ($null -ne $value) { [double]$value } })
        $blocked[$field] = [pscustomobject]@{
            maximum=$(if ($values.Count) { ($values | Measure-Object -Maximum).Maximum } else { $null })
            positive_snapshot_count=@($values | Where-Object { $_ -gt 0 }).Count
        }
    }
    $ages = @($diagnosticPolls | ForEach-Object { if ($null -ne $_.pipeline.maximum_publication_backlog_age_ms) { [double]$_.pipeline.maximum_publication_backlog_age_ms } elseif ($null -ne $_.pipeline.publication_backlog_age_ms) { [double]$_.pipeline.publication_backlog_age_ms } })
    $firstPoll = $diagnosticPolls | Select-Object -First 1
    $lastPoll = $diagnosticPolls | Select-Object -Last 1
    $counterRates = [ordered]@{}
    foreach ($field in @('completed_generation_tiles','boundary_preparation_completed_groups','publication_adopted_groups')) {
        $startValue = $null; $endValue = $null
        if ($null -ne $firstPoll -and $null -ne $lastPoll) {
            if ($field -eq 'completed_generation_tiles') {
                $startValue = $firstPoll.completed_generation_tiles; $endValue = $lastPoll.completed_generation_tiles
            } else {
                $startValue = $firstPoll.pipeline.$field; $endValue = $lastPoll.pipeline.$field
            }
        }
        if ($null -ne $startValue -and $null -ne $endValue -and $ElapsedSeconds -gt 0) {
            $delta = [double]$endValue-[double]$startValue
            $counterRates[$field] = [pscustomobject]@{ start=[double]$startValue; end=[double]$endValue; delta=$delta; per_second=$delta/$ElapsedSeconds; elapsed_seconds=$ElapsedSeconds; source='first and last poll counters divided by actual stopwatch elapsed time' }
        } else { $counterRates[$field] = $null }
    }
    $gaps = @()
    for ($i=1; $i -lt $Polls.Count; $i++) { $gaps += [double]$Polls[$i].elapsed_seconds-[double]$Polls[$i-1].elapsed_seconds }
    $throughputFields = @('generation_throughput_tiles_per_second','boundary_throughput_groups_per_second','publication_throughput_groups_per_second')
    $throughputDistributions = [ordered]@{}
    foreach ($field in $throughputFields) {
        $values = @($FrameSamples | ForEach-Object { $property=$_.PSObject.Properties[$field]; if ($null -ne $property -and $null -ne $property.Value) { [double]$property.Value } })
        $throughputDistributions[$field] = [pscustomobject]@{ sample_count=$values.Count; p50=(Get-Percentile $values 0.50); p95=(Get-Percentile $values 0.95); max=(Get-Percentile $values 1.0) }
    }
    [pscustomobject]@{
        diagnostic_poll_count=$diagnosticPolls.Count
        max_publication_backlog_age_ms=$(if ($ages.Count) { ($ages | Measure-Object -Maximum).Maximum } else { $null })
        blocked_causes=$blocked
        cumulative_counter_rates=$counterRates
        throughput_distributions=$throughputDistributions
        diagnostic_poll_gap_ms=[pscustomobject]@{ sample_count=$gaps.Count; p50=(Get-Percentile @($gaps | ForEach-Object { $_ * 1000.0 }) 0.50); p95=(Get-Percentile @($gaps | ForEach-Object { $_ * 1000.0 }) 0.95); max=(Get-Percentile @($gaps | ForEach-Object { $_ * 1000.0 }) 1.0) }
        diagnostics_sampling_scope='poll-to-poll wall interval; runtime/native frame samples retain finer per-frame throughput and pressure evidence'
    }
}
function Get-Percentile([double[]] $Values, [double] $Percentile) {
    $clean = @($Values | Where-Object { [double]::IsNaN($_) -eq $false -and [double]::IsInfinity($_) -eq $false } | Sort-Object)
    if ($clean.Count -eq 0) { return $null }
    $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $clean.Count) - 1)
    return [double]$clean[$index]
}
function Get-Distributions($Samples) {
    $fields = [ordered]@{
        interval_ms=@('interval_ms'); frame_cpu_ms=@('frame_cpu_ms'); update_ms=@('update_ms')
        terrain_update_ms=@('terrain_update_ms'); terrain_preparation_ms=@('terrain_preparation_ms')
        regional_advance_ms=@('regional_advance_ms'); selector_ms=@('selector_ms')
        publication_ms=@('publication_ms'); preparation_ms=@('preparation_ms')
        publication_collection_ms=@('publication_collection_ms'); publication_completion_ms=@('publication_completion_ms')
        publication_group_advance_ms=@('publication_group_advance_ms'); gpu_admission_ms=@('gpu_admission_ms')
        gpu_frontier_policy_comparison_ms=@('gpu_frontier_policy_comparison_ms')
        gpu_merge_frontiers_ms=@('gpu_merge_frontiers_ms')
        gpu_base_pins_ms=@('gpu_base_pins_ms')
        gpu_split_frontiers_ms=@('gpu_split_frontiers_ms')
        gpu_frontier_selection_ms=@('gpu_frontier_selection_ms')
        gpu_upload_allowlist_ms=@('gpu_upload_allowlist_ms')
        gpu_preparation_ms=@('gpu_preparation_ms'); gpu_terrain_ms=@('gpu_terrain_ms')
        host_frame_ms=@('host_frame_ms'); publication_admission_ms=@('publication_admission_ms')
        publication_budget_overruns=@('publication_budget_overruns')
        render_present_ms=@('render_present_ms'); diagnostics_ms=@('diagnostics_ms')
        completion_draining_ms=@('completion_draining_ms')
        publication_candidate_discovery_ms=@('publication_candidate_discovery_ms')
        publication_candidate_zone_ms=@('publication_candidate_zone_ms')
        publication_payload_collection_ms=@('publication_payload_collection_ms')
        publication_dispatch_ms=@('publication_dispatch_ms')
        publication_collection_unattributed_ms=@('publication_collection_unattributed_ms')
        publication_dependency_checks_ms=@('publication_dependency_checks_ms')
        publication_state_mutation_ms=@('publication_state_mutation_ms')
        publication_conflict_index_ms=@('publication_conflict_index_ms')
        resident_slot_allocation_ms=@('resident_slot_allocation_ms')
        resident_draw_preparation_ms=@('resident_draw_preparation_ms')
        boundary_preparation_background_ms=@('boundary_preparation_background_ms')
        gpu_validation_dependency_ms=@('gpu_validation_dependency_ms')
        gpu_resource_allocation_ms=@('gpu_resource_allocation_ms')
        gpu_upload_preparation_ms=@('gpu_upload_preparation_ms')
        gpu_drawable_metadata_ms=@('gpu_drawable_metadata_ms')
    }
    $result = [ordered]@{ sample_count=@($Samples | Where-Object { $null -ne $_ }).Count }
    foreach ($field in $fields.Keys) {
        $aliases = $fields[$field]
        $values = @($Samples | ForEach-Object { foreach ($alias in $aliases) { $v=$_.PSObject.Properties[$alias]; if ($null -ne $v -and $null -ne $v.Value) { [double]$v.Value; break } } })
        $result[$field] = [ordered]@{ p50=(Get-Percentile $values 0.50); p95=(Get-Percentile $values 0.95); max=$(if ($values.Count) { ($values | Measure-Object -Maximum).Maximum } else { $null }) }
    }
    return $result
}
function Get-Point($Observation, [string] $Phase) {
    $r = $Observation.resident
    $s = $Observation.snapshot
    $terrain = $s.terrain
    $camera = $s.camera
    [pscustomobject]@{
        elapsed_utc = [DateTime]::UtcNow.ToString('o')
        phase = $Phase
        frame_number = $s.general.frame_number
        camera = [pscustomobject]@{ mode=$s.general.camera_mode; clearance_m=$camera.terrain_clearance_m; drawn_mesh_clearance_m=$camera.drawn_mesh_clearance_m; position_m=$camera.position_m; orientation_xyzw=$camera.orientation_xyzw }
        terrain = [pscustomobject]@{ backend=$terrain.backend; settled=$terrain.settled; quality_pending=$terrain.quality_pending; ready=$terrain.ready; active_morph=$terrain.active_morph; construction_pending=$terrain.construction_pending; source_leaf_count=$terrain.source_leaf_count; visible_leaf_count=$terrain.visible_leaf_count }
        resident_planetary = Get-CompactResident $r
        export_scope = 'compact_poll'
        performance = $s.performance
    }
}
function Get-PerformancePoint($Observation, [string] $Phase, [double] $ElapsedSeconds) {
    $p = $Observation.snapshot.performance
    [pscustomobject]@{
        frame=$Observation.snapshot.general.frame_number
        phase=$Phase
        elapsed_seconds=[Math]::Round($ElapsedSeconds,3)
        frame_cpu_ms=$p.frame_cpu_ms
        host_frame_ms=$p.host_frame_ms
        update_ms=$p.update_ms
        terrain_update_ms=$p.terrain_update_ms
        preparation_ms=$p.preparation_ms
        terrain_preparation_ms=$p.terrain_preparation_ms
        gpu_terrain_ms=$p.gpu_terrain_ms
    }
}
function Invoke-ScheduledCheckpointCapture([string] $Name, [string] $Phase, [double] $ClearanceMeters, $RequestedOffset, [string] $CaptureKind, $Observation, $Watch) {
    Renew-Control
    $label = if ($CaptureKind -eq 'offset') { ([double]$RequestedOffset).ToString('0.0',[Globalization.CultureInfo]::InvariantCulture).Replace('.', 'p') + 's' } else { $CaptureKind }
    $captureName = "phase2e-$Name-$label"
    $response = Invoke-Dev @('capture',$script:lease,$captureName)
    $ActualOffset = $Watch.Elapsed.TotalSeconds
    $receiptPath = Join-Path $script:outputPath "$captureName-receipt.json"
    Write-JsonFile $receiptPath $response
    $data = $response.result.data.data
    $ok = $response.result.status -eq 'ok' -and $null -ne $data.snapshot -and $null -ne $data.viewport_image
    $captureSnapshot = $null
    if ($ok -and (Test-Path -LiteralPath $data.snapshot -PathType Leaf)) { $captureSnapshot = Get-Content -LiteralPath $data.snapshot -Raw | ConvertFrom-Json }
    [pscustomobject]@{
        name=$Name; route_phase=$Phase; target_clearance_m=$ClearanceMeters
        session_elapsed_seconds=[Math]::Round($script:sessionWatch.Elapsed.TotalSeconds,3)
        capture_kind=$CaptureKind
        requested_offset_seconds=$RequestedOffset; actual_offset_seconds=[Math]::Round($ActualOffset,3)
        scheduling_lateness_seconds=$(if ($null -ne $RequestedOffset) { [Math]::Round([Math]::Max(0,$ActualOffset-[double]$RequestedOffset),3) } else { $null })
        observed_frame=$Observation.snapshot.general.frame_number
        capture_frame=$(if ($null -ne $captureSnapshot) { $captureSnapshot.general.frame_number } else { $null })
        quality_pending=$Observation.snapshot.terrain.quality_pending; settled=$Observation.snapshot.terrain.settled
        status=$(if ($ok) { 'captured' } else { 'missing_pair' })
        receipt=$data; receipt_json=$receiptPath
        snapshot_sha256=$(if ($ok) { Get-HashOrNull $data.snapshot } else { $null })
        viewport_png_sha256=$(if ($ok) { Get-HashOrNull $data.viewport_image } else { $null })
        full_png_sha256=$(if ($ok) { Get-HashOrNull $data.full_image } else { $null })
    }
}
function Test-PoseStable($Previous, $Current, [double] $ToleranceMeters) {
    if ($null -eq $Previous -or $null -eq $Current) { return $false }
    $a = @($Previous.camera.position_m); $b = @($Current.camera.position_m)
    if ($a.Count -ne 3 -or $b.Count -ne 3) { return $false }
    $distance = [Math]::Sqrt([Math]::Pow($a[0]-$b[0],2)+[Math]::Pow($a[1]-$b[1],2)+[Math]::Pow($a[2]-$b[2],2))
    $qa=@($Previous.camera.orientation_xyzw); $qb=@($Current.camera.orientation_xyzw)
    $dot=0.0
    for ($i=0; $i -lt 4; $i++) { $dot += $qa[$i]*$qb[$i] }
    return ($distance -le $ToleranceMeters -and (1.0-[Math]::Abs($dot)) -le 1e-6)
}
function Invoke-Checkpoint([string] $Name, [double] $ClearanceMeters, [string] $Phase) {
    $checkpointSessionStart = $script:sessionWatch.Elapsed.TotalSeconds
    $script:activeStageSeries = [ordered]@{ name=$Name; route_phase=$Phase; target_clearance_m=$ClearanceMeters; completed=$false; before_observation=$null; convergence_samples=@(); warm_samples=@() }
    Send-Action @{ action='navigation_mode'; mode='surface_inspection' } "$Name-navigation-mode"
    $preAction = Get-Observation
    $script:lastObservation = $preAction
    $script:recentObservations = @(Get-Point $preAction 'failure_recent')
    $stageDirectory = Join-Path $script:outputPath 'stage-snapshots'
    New-Item -ItemType Directory -Path $stageDirectory -Force | Out-Null
    Renew-Control
    Write-JsonFile (Join-Path $stageDirectory "$Name-before.json") $preAction
    $script:activeStageSeries.before_observation = Get-Point $preAction 'before_action'
    $convergenceStartFrame = [long]$preAction.snapshot.general.frame_number
    Send-Action @{ action='clearance'; meters=$ClearanceMeters } "$Name-clearance"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $stableCount = 0; $idleStableCount = 0; $previousCompletedGeneration = $null; $previous = $null; $latest = $null; $lastRenewal = [DateTime]::UtcNow
    $usefulMs = $null; $usefulDetailMs = $null; $settledMs = $null; $qualityTimedOut = $false
    $constructionIdleMs = $null
    $observations = @(); $pollRecords = @(); $convergenceFrameMap=@{}; $convergencePerformanceMap=@{}
    $scheduledCaptures = @()
    $settledCaptureRecord = $null
    $script:checkpointCaptureSchedule = [bool]$CaptureCheckpoints
    $script:checkpointCaptureOffsets = @($CheckpointCaptureOffsetsSeconds | Sort-Object -Unique)
    $script:checkpointCaptureIndex = 0
    while ($watch.Elapsed.TotalSeconds -lt $script:CheckpointTimeoutSeconds) {
        if ($null -ne $script:nativeProcess -and $script:nativeProcess.HasExited) { throw "Native app exited with code $($script:nativeProcess.ExitCode)." }
        $obs = Get-Observation; $latest = $obs
        $script:lastObservation = $obs
        $script:recentObservations += (Get-Point $obs 'failure_recent')
        if ($script:recentObservations.Count -gt 64) { $script:recentObservations = @($script:recentObservations | Select-Object -Last 64) }
        $point = Get-Point $obs 'convergence'
        $observations += $point
        $r = $obs.resident
        $performancePoint = Get-PerformancePoint $obs 'convergence' $watch.Elapsed.TotalSeconds
        if ($null -ne $performancePoint.frame -and [long]$performancePoint.frame -gt $convergenceStartFrame) { $convergencePerformanceMap[[string]$performancePoint.frame] = $performancePoint }
        foreach ($frameSample in @(Get-FrameSamples $r)) { if ($null -ne $frameSample.frame -and [long]$frameSample.frame -gt $convergenceStartFrame) { $convergenceFrameMap[[string]$frameSample.frame] = $frameSample } }
        $drawable=$null; $desired=$null; $ifUseful=$false
        if ($null -ne $r) {
            $drawable = Get-Number $r @('drawable_count')
            $desired = Get-Number $r @('desired_count')
            $coverage = if ($null -ne $r.has_coverage) { $r.has_coverage } elseif ($null -ne $r.useful_coverage) { $r.useful_coverage } else { $null }
            $ifUseful = if ($null -ne $coverage) { $coverage -eq $true } else { ($null -ne $drawable -and $drawable -ge 6) }
        } elseif ($script:LegacyTerrain) {
            $visible = [double]$obs.snapshot.terrain.visible_leaf_count
            $source = [double]$obs.snapshot.terrain.source_leaf_count
            $ifUseful = ($obs.snapshot.terrain.ready -eq $true -and ($visible -gt 0 -or $source -gt 0))
        }
        if ($ifUseful -and $null -eq $usefulMs) { $usefulMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3) }
        if ($null -ne $r -and $r.useful_detail_reached -eq $true -and $null -eq $usefulDetailMs) { $usefulDetailMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3) }
        if ($obs.snapshot.terrain.settled -eq $true -and $null -eq $settledMs) { $settledMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3) }
        $pipelinePoint = Get-PipelineSnapshot $r
        $pollRecords += [pscustomobject]@{
            elapsed_seconds=[Math]::Round($watch.Elapsed.TotalSeconds,3); schema_available=($null -ne $r)
            frame=$obs.snapshot.general.frame_number; useful_coverage=$ifUseful; desired_count=$desired; drawable_count=$drawable
            useful_detail_reached=$(if ($null -ne $r) { $r.useful_detail_reached } else { $null })
            visible_drawable_proxy_error_max_px=$(if ($null -ne $r) { $r.visible_drawable_proxy_error_max_px } else { $null })
            visible_drawable_proxy_error_p95_px=$(if ($null -ne $r) { $r.visible_drawable_proxy_error_p95_px } else { $null })
            useful_detail_proxy_error_threshold_px=$(if ($null -ne $r) { $r.useful_detail_proxy_error_threshold_px } else { $null })
            useful_detail_proxy_error_certified=$(if ($null -ne $r) { $r.visible_proxy_error_certified } else { $null })
            camera_mode=$obs.snapshot.general.camera_mode; actual_clearance_m=$obs.snapshot.camera.terrain_clearance_m
            position_m=$obs.snapshot.camera.position_m; orientation_xyzw=$obs.snapshot.camera.orientation_xyzw
            jobs_started=(Get-Number $r @('jobs_started')); generation_throughput_tiles_per_second=(Get-Number $r @('generation_throughput_tiles_per_second'))
            tile_upload_bytes_per_frame=(Get-Number $r @('tile_upload_bytes','gpu_upload_bytes_per_frame','tile_upload_bytes_per_frame'))
            boundary_upload_bytes_per_frame=(Get-Number $r @('boundary_upload_bytes','boundary_upload_bytes_per_frame'))
            completed_generation_tiles=(Get-Number $r @('completed_generation_tiles'))
            pipeline=$pipelinePoint
            engine_profile=$obs.snapshot.performance.engine_profile
            terrain_trace=$(if ($null -ne $r) { Get-CompactTerrainTrace $r.terrain_trace } else { $null })
            quality_pending=$obs.snapshot.terrain.quality_pending; settled=$obs.snapshot.terrain.settled
            performance=$performancePoint
        }
        $script:activeStageSeries.convergence_samples = $pollRecords
        $stable = $false
        if ($null -ne $previous -and $obs.snapshot.general.camera_mode -eq 'surface_inspection') {
            $stable = Test-PoseStable $previous $point ([Math]::Max(0.25, $ClearanceMeters * 0.0002))
        }
        if ($stable) { $stableCount++ } else { $stableCount=0 }
        $completedGeneration = Get-Number $r @('completed_generation_tiles')
        if ($obs.snapshot.terrain.settled -eq $true -and $obs.snapshot.terrain.construction_pending -eq $false) {
            if ($script:LegacyTerrain) { $idleStableCount++ }
            elseif ($null -ne $completedGeneration -and $null -ne $previousCompletedGeneration -and $completedGeneration -eq $previousCompletedGeneration) { $idleStableCount++ }
            else { $idleStableCount=0 }
        } else { $idleStableCount=0 }
        if ($idleStableCount -ge 3 -and $null -eq $constructionIdleMs) { $constructionIdleMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3) }
        $previousCompletedGeneration = $completedGeneration
        $previous = $point
        while ($script:checkpointCaptureSchedule -and $script:checkpointCaptureIndex -lt $script:checkpointCaptureOffsets.Count -and $watch.Elapsed.TotalSeconds -ge [double]$script:checkpointCaptureOffsets[$script:checkpointCaptureIndex]) {
            $offset = [double]$script:checkpointCaptureOffsets[$script:checkpointCaptureIndex]
            $scheduledCaptures += Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $offset 'offset' $obs $watch
            $script:checkpointCaptureIndex++
        }
        if ($script:checkpointCaptureSchedule -and $null -ne $settledMs -and $null -eq $settledCaptureRecord) {
            $settledCaptureRecord = Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $null 'settled' $obs $watch
            $scheduledCaptures += $settledCaptureRecord
        }
        if ($stableCount -ge 3 -and $idleStableCount -ge 3) { break }
        Renew-Control
        Start-Sleep -Seconds $script:PollSeconds
    }
    if ($stableCount -lt 3) {
        Renew-Control
        $convergenceFrames = @($convergenceFrameMap.Values | Sort-Object { [long]$_.frame })
        $convergencePerf = @($convergencePerformanceMap.Values | Sort-Object { [long]$_.frame })
        $failedCheckpoint = [pscustomobject]@{ name=$Name; route_phase=$Phase; target_clearance_m=$ClearanceMeters; session_start_elapsed_seconds=[Math]::Round($checkpointSessionStart,3); session_end_elapsed_seconds=[Math]::Round($script:sessionWatch.Elapsed.TotalSeconds,3); camera_transition_stopped=$false; timeout_seconds=$script:CheckpointTimeoutSeconds; useful_coverage_elapsed_ms=$usefulMs; useful_detail_elapsed_ms=$usefulDetailMs; settled_quality_elapsed_ms=$settledMs; convergence_samples=$pollRecords; convergence_frames=$convergenceFrames; convergence_performance_samples=$convergencePerf; convergence_frame_distributions=(Get-Distributions $(if ($convergenceFrames.Count) { $convergenceFrames } else { $convergencePerf })); convergence_pipeline_summary=(Get-PipelineSummary $pollRecords $convergenceFrames $watch.Elapsed.TotalSeconds); resident_planetary=(Get-CompactResident $latest.resident); failure='camera_transition_timeout' }
        $script:summary.checkpoints += $failedCheckpoint
        Renew-Control
        Write-ProgressFile
        throw "Camera did not stop transitioning at checkpoint '$Name' within $($script:CheckpointTimeoutSeconds)s."
    }
    if ($latest.snapshot.terrain.settled -ne $true) { $qualityTimedOut = $true }

    # Capture all requested offsets even when quality settles before the last one.
    while ($script:checkpointCaptureSchedule -and $script:checkpointCaptureIndex -lt $script:checkpointCaptureOffsets.Count) {
        $nextOffset = [double]$script:checkpointCaptureOffsets[$script:checkpointCaptureIndex]
        if ($watch.Elapsed.TotalSeconds -ge $nextOffset) {
            $scheduledCaptures += Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $nextOffset 'offset' $latest $watch
            $script:checkpointCaptureIndex++
            continue
        }
        Renew-Control
        $remaining = $nextOffset - $watch.Elapsed.TotalSeconds
        Start-Sleep -Seconds ([Math]::Min($script:PollSeconds, [Math]::Max(0.05, $remaining)))
        $latest = Get-Observation
        $script:lastObservation = $latest
        $script:recentObservations += (Get-Point $latest 'failure_recent')
        if ($script:recentObservations.Count -gt 64) { $script:recentObservations = @($script:recentObservations | Select-Object -Last 64) }
        while ($script:checkpointCaptureIndex -lt $script:checkpointCaptureOffsets.Count -and $watch.Elapsed.TotalSeconds -ge [double]$script:checkpointCaptureOffsets[$script:checkpointCaptureIndex]) {
            $offset = [double]$script:checkpointCaptureOffsets[$script:checkpointCaptureIndex]
            $scheduledCaptures += Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $offset 'offset' $latest $watch
            $script:checkpointCaptureIndex++
        }
        if ($latest.snapshot.terrain.settled -eq $true -and $null -eq $settledCaptureRecord) {
            $settledMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3)
            $settledCaptureRecord = Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $null 'settled' $latest $watch
            $scheduledCaptures += $settledCaptureRecord
        }
    }
    Renew-Control
    Write-JsonFile (Join-Path $stageDirectory "$Name-after.json") $latest

    $warmStartObservation = $latest
    $warmStartFrame = [long]$warmStartObservation.snapshot.general.frame_number
    $startCompletedGeneration = Get-Number $warmStartObservation.resident @('completed_generation_tiles')
    $startJobs = Get-Number $warmStartObservation.resident @('jobs_started')
    $startTileUploads = Get-Number $warmStartObservation.resident @('cumulative_tile_upload_bytes','gpu_cumulative_tile_upload_bytes','gpu_cumulative_content_upload_bytes','cumulative_content_upload_bytes','tile_content_upload_bytes_total')
    $startBoundaryUploads = Get-Number $warmStartObservation.resident @('cumulative_boundary_upload_bytes','gpu_cumulative_boundary_upload_bytes','boundary_cumulative_upload_bytes')
    $warmWatch = [Diagnostics.Stopwatch]::StartNew(); $warmObservations=@(); $warmPolls=@(); $warmFrameMap=@{}; $warmPerformanceMap=@{}; $lastRenewal=[DateTime]::UtcNow
    while ($warmWatch.Elapsed.TotalSeconds -lt $script:WarmSeconds) {
        $obs = Get-Observation; $warmObservations += $obs
        $script:lastObservation = $obs
        $script:recentObservations += (Get-Point $obs 'failure_recent')
        if ($script:recentObservations.Count -gt 64) { $script:recentObservations = @($script:recentObservations | Select-Object -Last 64) }
        if ($script:checkpointCaptureSchedule -and $obs.snapshot.terrain.settled -eq $true -and $null -eq $settledCaptureRecord) {
            $settledMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3)
            $settledCaptureRecord = Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $null 'settled' $obs $watch
            $scheduledCaptures += $settledCaptureRecord
        }
        $warmPolls += Get-Point $obs 'warm'
        $script:activeStageSeries.warm_samples = $warmPolls
        $r = $obs.resident
        $pipelinePoint = Get-PipelineSnapshot $r
        $performancePoint = Get-PerformancePoint $obs 'warm' $warmWatch.Elapsed.TotalSeconds
        if ($null -ne $performancePoint.frame) { $warmPerformanceMap[[string]$performancePoint.frame] = $performancePoint }
        foreach ($frameSample in @(Get-FrameSamples $r)) { if ($null -ne $frameSample.frame) { $warmFrameMap[[string]$frameSample.frame] = $frameSample } }
        foreach ($sampleProperty in $performancePoint.PSObject.Properties) {
            if ($sampleProperty.Name -notin @('frame','phase','elapsed_seconds')) { $warmPolls[-1] | Add-Member -NotePropertyName "cpu_$($sampleProperty.Name)" -NotePropertyValue $sampleProperty.Value -Force }
        }
        $warmPolls[-1] | Add-Member -NotePropertyName jobs_started -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('jobs_started') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName completed_generation_tiles -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('completed_generation_tiles') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName useful_detail_reached -NotePropertyValue $(if ($null -ne $r) { $r.useful_detail_reached } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName useful_detail_proxy_error_threshold_px -NotePropertyValue $(if ($null -ne $r) { $r.useful_detail_proxy_error_threshold_px } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName pipeline -NotePropertyValue $pipelinePoint
        $warmPolls[-1] | Add-Member -NotePropertyName engine_profile -NotePropertyValue $obs.snapshot.performance.engine_profile
        $warmPolls[-1] | Add-Member -NotePropertyName terrain_trace -NotePropertyValue $(if ($null -ne $r) { Get-CompactTerrainTrace $r.terrain_trace } else { $null })
        if ($null -eq $usefulDetailMs -and $null -ne $r -and $r.useful_detail_reached -eq $true) { $usefulDetailMs = [Math]::Round($watch.Elapsed.TotalMilliseconds,3) }
        $warmPolls[-1] | Add-Member -NotePropertyName construction_pending -NotePropertyValue $obs.snapshot.terrain.construction_pending
        $warmPolls[-1] | Add-Member -NotePropertyName worker_queued -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('worker_queued') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName worker_running -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('worker_running') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName completion_backlog -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('completion_backlog') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName generation_throughput_tiles_per_second -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('generation_throughput_tiles_per_second') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName tile_upload_bytes_per_frame -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('tile_upload_bytes','gpu_upload_bytes_per_frame','tile_upload_bytes_per_frame') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName boundary_upload_bytes_per_frame -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('boundary_upload_bytes','boundary_upload_bytes_per_frame') } else { $null })
        $warmPolls[-1] | Add-Member -NotePropertyName jobs_queued -NotePropertyValue $(if ($null -ne $r) { Get-Number $r @('worker_queued') } else { $null })
        if ($warmWatch.Elapsed.TotalSeconds -lt $script:WarmSeconds) {
            Renew-Control
            Start-Sleep -Seconds ([Math]::Min($script:PollSeconds, [Math]::Max(0.1, $script:WarmSeconds-$warmWatch.Elapsed.TotalSeconds)))
        }
    }
    $latest = $warmObservations[-1]
    $warmEndFrame = [long]$latest.snapshot.general.frame_number
    $allFrames = @($warmFrameMap.Values | Sort-Object { [long]$_.frame })
    $windowFrames = @($allFrames | Where-Object { $_.frame -gt $warmStartFrame -and $_.frame -le $warmEndFrame })
    $expectedFrameCount = [Math]::Max(0, $warmEndFrame - $warmStartFrame)
    $observedFrameCount = $windowFrames.Count
    Renew-Control
    $warmSampleCoverage = [pscustomobject]@{
        expected_frame_count=$expectedFrameCount
        observed_frame_count=$observedFrameCount
        missing_frame_count=[Math]::Max(0,$expectedFrameCount-$observedFrameCount)
        completion_ratio=$(if ($expectedFrameCount -gt 0) { [Math]::Min(1.0,[double]$observedFrameCount/[double]$expectedFrameCount) } else { $null })
        interval_distributions=(Get-Distributions $windowFrames)
        definition='observed distinct resident native samples in warm frame-number interval divided by frame-number advance; null when no frame advance'
    }
    $warmPerformanceSamples = @($warmPerformanceMap.Values | Sort-Object { [long]$_.frame })
    $distributions = if ($windowFrames.Count) { Get-Distributions $windowFrames } else { Get-Distributions $warmPerformanceSamples }
    $endJobs = Get-Number $latest.resident @('jobs_started')
    $endTileUploads = Get-Number $latest.resident @('cumulative_tile_upload_bytes','gpu_cumulative_tile_upload_bytes','gpu_cumulative_content_upload_bytes','cumulative_content_upload_bytes','tile_content_upload_bytes_total')
    $endBoundaryUploads = Get-Number $latest.resident @('cumulative_boundary_upload_bytes','gpu_cumulative_boundary_upload_bytes','boundary_cumulative_upload_bytes')
    $endCompletedGeneration = Get-Number $latest.resident @('completed_generation_tiles')
    $tileUploadFrameBytes = @($windowFrames | ForEach-Object { if ($null -ne $_.tile_upload_bytes) { [double]$_.tile_upload_bytes } })
    $boundaryUploadFrameBytes = @($windowFrames | ForEach-Object { if ($null -ne $_.boundary_upload_bytes) { [double]$_.boundary_upload_bytes } })
    $metadataUploadFrameBytes = @($windowFrames | ForEach-Object { if ($null -ne $_.metadata_upload_bytes) { [double]$_.metadata_upload_bytes } })
    $convergenceFrames = @($convergenceFrameMap.Values | Sort-Object { [long]$_.frame })
    $convergencePerformanceSamples = @($convergencePerformanceMap.Values | Sort-Object { [long]$_.frame })
    $convergenceFrameSource = if ($convergenceFrames.Count) { 'resident_planetary.native_frame_samples' } else { 'snapshot.performance_poll' }
    $convergenceFrameDistributionSamples = if ($convergenceFrames.Count) { $convergenceFrames } else { $convergencePerformanceSamples }
    $captureRecord = $settledCaptureRecord
    if ($script:checkpointCaptureSchedule -and $null -eq $captureRecord) {
        $captureRecord = Invoke-ScheduledCheckpointCapture $Name $Phase $ClearanceMeters $null 'final-state' $latest $watch
        $scheduledCaptures += $captureRecord
    }
    if ($script:checkpointCaptureSchedule) {
        $captureData = $captureRecord.receipt
        $captureSnapshot = if ($null -ne $captureRecord.receipt.snapshot -and (Test-Path -LiteralPath $captureRecord.receipt.snapshot -PathType Leaf)) { Get-Content -LiteralPath $captureRecord.receipt.snapshot -Raw | ConvertFrom-Json } else { $null }
    } else {
        $captureName = "planetary-$Name"
        Renew-Control
        $capture = Invoke-Dev @('capture',$script:lease,$captureName)
        Write-JsonFile (Join-Path $script:outputPath "$captureName-receipt.json") $capture
        $captureData = $capture.result.data.data
        if ($capture.result.status -ne 'ok' -or $null -eq $captureData.snapshot -or $null -eq $captureData.viewport_image) { throw "Capture '$captureName' did not produce a paired native bundle." }
        $captureSnapshot = Get-Content -LiteralPath $captureData.snapshot -Raw | ConvertFrom-Json
    }
    Renew-Control
    $backend = [string]$captureSnapshot.terrain.backend
    $expectedBackend = if ($script:LegacyTerrain) { 'LEGACY CPU MESH' } else { 'RESIDENT TILE' }
    $convergencePipelineSummary = Get-PipelineSummary $pollRecords $convergenceFrames $watch.Elapsed.TotalSeconds
    $warmPipelineSummary = Get-PipelineSummary $warmPolls $windowFrames $warmWatch.Elapsed.TotalSeconds
    $checkpoint = [pscustomobject]@{
        name=$Name; route_phase=$Phase; target_clearance_m=$ClearanceMeters
        session_start_elapsed_seconds=[Math]::Round($checkpointSessionStart,3)
        session_end_elapsed_seconds=[Math]::Round($script:sessionWatch.Elapsed.TotalSeconds,3)
        camera_transition_stopped=$true; camera_mode=$latest.snapshot.general.camera_mode
        actual_clearance_m=$latest.snapshot.camera.terrain_clearance_m
        useful_coverage_elapsed_ms=$usefulMs; useful_coverage_observed=($null -ne $usefulMs)
        useful_coverage_threshold=$(if ($script:LegacyTerrain) { 'terrain.ready == true and visible_leaf_count > 0 or source_leaf_count > 0' } elseif ($null -ne $latest.resident.has_coverage) { 'resident_planetary.has_coverage == true' } elseif ($null -ne $latest.resident.useful_coverage) { 'resident_planetary.useful_coverage == true' } else { 'resident_planetary.drawable_count >= 6' })
        useful_detail_elapsed_ms=$usefulDetailMs; useful_detail_reached=($latest.resident.useful_detail_reached -eq $true)
        useful_detail_proxy_error_threshold_px=$latest.resident.useful_detail_proxy_error_threshold_px
        useful_detail_proxy_error_certified=$latest.resident.visible_proxy_error_certified
        useful_detail_metric='resident_planetary.useful_detail_reached; approximate selector projected-error proxy; uncertified and separate from visual acceptance'
        settled_quality_elapsed_ms=$settledMs; settled_quality=($latest.snapshot.terrain.settled -eq $true)
        construction_idle_elapsed_ms=$constructionIdleMs; construction_idle_observed=($null -ne $constructionIdleMs)
        quality_timeout=$qualityTimedOut; timeout_seconds=$script:CheckpointTimeoutSeconds
        requested_quality=$latest.snapshot.terrain.desired_radial_lod; displayed_quality=$latest.snapshot.terrain.ready_radial_lod
        terrain=$latest.snapshot.terrain; resident_planetary=(Get-CompactResident $latest.resident)
        convergence_samples=$pollRecords; convergence_frames=$convergenceFrames
        convergence_performance_samples=$convergencePerformanceSamples
        convergence_frame_source=$convergenceFrameSource
        convergence_frame_distributions=(Get-Distributions $convergenceFrameDistributionSamples)
        convergence_pipeline_summary=$convergencePipelineSummary
        scheduled_captures=$scheduledCaptures
        warm_pipeline_summary=$warmPipelineSummary
        warm_samples=$warmPolls; warm_frames=$windowFrames; warm_performance_samples=$warmPerformanceSamples
        warm_interval_seconds=[Math]::Round($warmWatch.Elapsed.TotalSeconds,3)
        warm_frame_range=@($warmStartFrame,$warmEndFrame); frame_sample_count=$windowFrames.Count
        warm_sample_coverage=$warmSampleCoverage
        frame_distributions=$distributions
        warm_performance_distributions=(Get-Distributions $warmPerformanceSamples)
        warm_deltas=[pscustomobject]@{
            jobs_started=$(if ($null -ne $startJobs -and $null -ne $endJobs) { $endJobs-$startJobs } else { $null })
            completed_generation_tiles=$(if ($null -ne $startCompletedGeneration -and $null -ne $endCompletedGeneration) { $endCompletedGeneration-$startCompletedGeneration } else { $null })
            cumulative_tile_content_upload_bytes=$(if ($null -ne $startTileUploads -and $null -ne $endTileUploads) { $endTileUploads-$startTileUploads } else { $null })
            cumulative_boundary_upload_bytes=$(if ($null -ne $startBoundaryUploads -and $null -ne $endBoundaryUploads) { $endBoundaryUploads-$startBoundaryUploads } else { $null })
            sampled_tile_content_upload_bytes=if ($tileUploadFrameBytes.Count) { ($tileUploadFrameBytes | Measure-Object -Sum).Sum } else { $null }
            sampled_boundary_upload_bytes=if ($boundaryUploadFrameBytes.Count) { ($boundaryUploadFrameBytes | Measure-Object -Sum).Sum } else { $null }
            sampled_metadata_upload_bytes=if ($metadataUploadFrameBytes.Count) { ($metadataUploadFrameBytes | Measure-Object -Sum).Sum } else { $null }
            max_sampled_tile_upload_bytes_per_frame=Get-Percentile $tileUploadFrameBytes 1.0
            max_sampled_boundary_upload_bytes_per_frame=Get-Percentile $boundaryUploadFrameBytes 1.0
            generation_throughput_tiles_per_second=(Get-Number $latest.resident @('generation_throughput_tiles_per_second'))
        }
        backend=$backend; expected_backend=$expectedBackend; backend_matches=($backend -eq $expectedBackend)
        capture=[pscustomobject]@{ receipt=$captureData; sample_record=$captureRecord; snapshot_sha256=Get-HashOrNull $captureData.snapshot; viewport_png_sha256=Get-HashOrNull $captureData.viewport_image; full_png_sha256=Get-HashOrNull $captureData.full_image }
    }
    $script:summary.checkpoints += $checkpoint
    $script:activeStageSeries.completed = $true
    Renew-Control
    Write-ProgressFile
}

if ($FingerprintOnly) {
    try {
        $script:ExecutablePath = if ([IO.Path]::IsPathRooted($ExecutablePath)) { [IO.Path]::GetFullPath($ExecutablePath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $ExecutablePath)) }
        $script:DeveloperCliPath = if ([IO.Path]::IsPathRooted($DeveloperCliPath)) { [IO.Path]::GetFullPath($DeveloperCliPath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $DeveloperCliPath)) }
        $fingerprints = Get-Fingerprints
        Write-Output (ConvertTo-Json -InputObject $fingerprints -Depth 20)
    } finally {
        Pop-Location
    }
    return
}

try {
    $script:ExecutablePath = if ([IO.Path]::IsPathRooted($ExecutablePath)) { [IO.Path]::GetFullPath($ExecutablePath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $ExecutablePath)) }
    $script:DeveloperCliPath = if ([IO.Path]::IsPathRooted($DeveloperCliPath)) { [IO.Path]::GetFullPath($DeveloperCliPath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $DeveloperCliPath)) }
    if (-not (Test-Path -LiteralPath $script:ExecutablePath -PathType Leaf)) { throw "App executable not found: $script:ExecutablePath" }
    if (-not (Test-Path -LiteralPath $script:DeveloperCliPath -PathType Leaf)) { throw "Developer CLI not found: $script:DeveloperCliPath" }
    if ([string]::IsNullOrWhiteSpace($EvidenceDirectory)) { $EvidenceDirectory = Join-Path '../target/terrain-redesign/slice2d' ([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '-' + [Guid]::NewGuid().ToString('N').Substring(0,8)) }
    $script:outputPath = if ([IO.Path]::IsPathRooted($EvidenceDirectory)) { [IO.Path]::GetFullPath($EvidenceDirectory) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $EvidenceDirectory)) }
    if (Test-Path -LiteralPath $script:outputPath) { throw "Evidence directory already exists: $script:outputPath" }
    New-Item -ItemType Directory -Path $script:outputPath -Force | Out-Null
    if ([string]::IsNullOrWhiteSpace($SessionId)) {
        $script:registryPath = Join-Path $script:outputPath 'registry'
    } else {
        if ([string]::IsNullOrWhiteSpace($RegistryPath)) { throw 'Supply -RegistryPath when attaching to an already-running session.' }
        $script:registryPath = if ([IO.Path]::IsPathRooted($RegistryPath)) { [IO.Path]::GetFullPath($RegistryPath) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $RegistryPath)) }
    }
    New-Item -ItemType Directory -Path $script:registryPath -Force | Out-Null
    $summary.fingerprints = Get-Fingerprints
    $summary.runner_script = $script:RunnerScriptPath
    $summary.runner_script_sha256 = Get-HashOrNull $script:RunnerScriptPath
    $summary.capture_directory = if ($DisableAutomaticCaptures) { $null } else { Join-Path $script:outputPath 'automatic-captures' }
    $summary.sampling = [pscustomobject]@{ poll_interval_seconds=$PollSeconds; checkpoint_timeout_seconds=$CheckpointTimeoutSeconds; warm_seconds=$WarmSeconds; native_worker_delays='normal'; capture_schedule_enabled=[bool]$CaptureCheckpoints; automatic_captures_enabled=(-not [bool]$DisableAutomaticCaptures) }
    Write-JsonFile (Join-Path $script:outputPath 'baseline.json') $summary.fingerprints
    if ([string]::IsNullOrWhiteSpace($SessionId)) {
        $oldRegistry = $env:MUNDARIS_DEV_REGISTRY
        $oldOutput = $env:MUNDARIS_DEV_OUTPUT
        $oldCaptureDirectory = $env:MUNDARIS_CAPTURE_DIR
        $env:MUNDARIS_DEV_REGISTRY = $script:registryPath
        $nativeEvidence = Join-Path $script:outputPath 'native-captures'
        New-Item -ItemType Directory -Path $nativeEvidence | Out-Null
        $env:MUNDARIS_DEV_OUTPUT = $nativeEvidence
        if ($DisableAutomaticCaptures) {
            Remove-Item Env:MUNDARIS_CAPTURE_DIR -ErrorAction SilentlyContinue
        } else {
            $env:MUNDARIS_CAPTURE_DIR = Join-Path $script:outputPath 'automatic-captures'
            New-Item -ItemType Directory -Path $env:MUNDARIS_CAPTURE_DIR -Force | Out-Null
        }
        try {
            $arguments = @('--solar-system','--dev-interface')
            if ($LegacyTerrain) { $arguments += '--legacy-terrain' }
            $nativeProcess = Start-Process -FilePath $script:ExecutablePath -ArgumentList $arguments -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $script:outputPath 'native.stdout.log') -RedirectStandardError (Join-Path $script:outputPath 'native.stderr.log')
        } finally { $env:MUNDARIS_DEV_REGISTRY = $oldRegistry; $env:MUNDARIS_DEV_OUTPUT = $oldOutput; $env:MUNDARIS_CAPTURE_DIR = $oldCaptureDirectory }
        $script:sessionId = $null
        $startup = [Diagnostics.Stopwatch]::StartNew()
        while ($startup.Elapsed.TotalSeconds -lt $StartupTimeoutSeconds) {
            if ($nativeProcess.HasExited) { throw "Native app exited during startup with code $($nativeProcess.ExitCode). See native.stderr.log." }
            $text = & $script:DeveloperCliPath --registry $script:registryPath sessions 2>$null
            if ($LASTEXITCODE -eq 0) {
                $found = (($text -join "`n") | ConvertFrom-Json).sessions | Where-Object { [int]$_.pid -eq $nativeProcess.Id }
                if (@($found).Count -eq 1) { $session = $found; $script:sessionId=[string]$session.session_id; break }
            }
            Start-Sleep -Milliseconds 400
        }
        if ($null -eq $session) { throw "Developer session did not register within $StartupTimeoutSeconds seconds." }
    } else {
        $script:sessionId = $SessionId
        $sessionsText = & $script:DeveloperCliPath --registry $script:registryPath sessions 2>$null
        if ($LASTEXITCODE -ne 0) { throw 'Could not inspect the supplied native session.' }
        $session = ((($sessionsText -join "`n") | ConvertFrom-Json).sessions | Where-Object session_id -EQ $SessionId | Select-Object -First 1)
        if ($null -eq $session) { throw "Session '$SessionId' is not live in the evidence registry. For an existing session, supply its registry with -RegistryPath in the current script version." }
    }
    $summary.session = [pscustomobject]@{ session_id=$script:sessionId; pid=$session.pid; preset=$session.preset; executable=$session.executable; executable_sha256=$session.binary_sha256; output_directory=$session.output_directory; owned_process=($null -ne $nativeProcess); legacy_launch_flag=[bool]$LegacyTerrain }
    if ($ClientWidth -gt 0) {
        if ($null -eq $nativeProcess) { throw 'Window resizing is restricted to this runner-owned process.' }
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PlanetaryEvidenceWindow {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window, IntPtr after, int x, int y, int width, int height, uint flags);
}
'@
        $nativeProcess.Refresh()
        $window = $nativeProcess.MainWindowHandle
        if ($window -eq [IntPtr]::Zero) { throw 'Owned native window handle unavailable.' }
        $outer = New-Object PlanetaryEvidenceWindow+Rect
        $client = New-Object PlanetaryEvidenceWindow+Rect
        if (![PlanetaryEvidenceWindow]::GetWindowRect($window, [ref]$outer) -or
            ![PlanetaryEvidenceWindow]::GetClientRect($window, [ref]$client)) { throw 'Cannot measure owned window.' }
        $width = $ClientWidth + ($outer.Right - $outer.Left) - ($client.Right - $client.Left)
        $height = $ClientHeight + ($outer.Bottom - $outer.Top) - ($client.Bottom - $client.Top)
        $after = if ($KeepBackground) { [IntPtr]::new(1) } else { [IntPtr]::Zero }
        $flags = if ($KeepBackground) { 0x12 } else { 0x16 }
        if (![PlanetaryEvidenceWindow]::SetWindowPos($window, $after, 0, 0, $width, $height, $flags)) { throw 'Cannot resize owned window.' }
        Start-Sleep -Milliseconds 250
    }
    if ($session.preset -notin @('solar-system','real-solar-system')) { throw "Expected a Solar System session; found preset '$($session.preset)'." }
    $control = Invoke-Dev @('control','acquire','slice2d-planetary-evidence')
    if ($control.status -ne 'ok') { throw "Could not acquire native control lease: $($control | ConvertTo-Json -Compress -Depth 5)" }
    $lease = [string]$control.data.lease
    if ([string]::IsNullOrWhiteSpace($lease)) { $lease=[string]$control.data.token }
    if ([string]::IsNullOrWhiteSpace($lease)) { throw 'Control lease response did not include a token.' }
    $initialResponse = Invoke-Dev @('inspect')
    $initial = Get-Observation
    $moon = @($initialResponse.data.bodies | Where-Object { $_.name -eq 'Moon' })
    if ($moon.Count -eq 0) { throw 'The selected Solar System session did not expose the real Moon in its body inventory.' }
    $moonHandle = [string]$moon[0].handle
    if ([string]::IsNullOrWhiteSpace($moonHandle)) { throw 'Moon body inventory did not include its opaque control handle.' }
    if ([string]::IsNullOrWhiteSpace([string]$initial.snapshot.terrain.backend)) { throw 'Terrain backend field is missing; refusing an unclassified comparison.' }
    $bootstrapFrameMap = @{}
    $bootstrapObservations = @($initial)
    foreach ($startupAction in @(
        @{name='pause-simulation'; command=@{action='pause'; paused=$true}},
        @{name='focus-moon'; command=@{action='focus'; body=$moonHandle; body_fixed=$true}},
        @{name='look-at-moon'; command=@{action='look_at'; body=$moonHandle}},
        @{name='surface-inspection-mode'; command=@{action='navigation_mode'; mode='surface_inspection'}}
    )) {
        Send-Action $startupAction.command $startupAction.name
        $bootstrapObservations += Get-Observation
    }
    foreach ($bootstrapObservation in $bootstrapObservations) {
        $bodyIndex = $bootstrapObservation.snapshot.terrain.active_body.index
        foreach ($sample in @(Get-FrameSamples $bootstrapObservation.resident)) {
            $sample | Add-Member -NotePropertyName bootstrap_body_index -NotePropertyValue $bodyIndex -Force
            $bootstrapFrameMap["${bodyIndex}:$($sample.frame)"] = $sample
        }
    }
    $summary.bootstrap_frames = @($bootstrapFrameMap.Values | Sort-Object bootstrap_body_index,frame)
    $summary.bootstrap_frame_distributions = Get-Distributions $summary.bootstrap_frames
    $route = @(
        @{ name='approach-1000km'; clearance=1000000.0; phase='approach' },
        @{ name='approach-500km'; clearance=500000.0; phase='approach' },
        @{ name='approach-250km'; clearance=250000.0; phase='approach' },
        @{ name='approach-100km'; clearance=100000.0; phase='approach' },
        @{ name='approach-25km'; clearance=25000.0; phase='approach' },
        @{ name='approach-5km'; clearance=5000.0; phase='approach' },
        @{ name='approach-1km'; clearance=1000.0; phase='approach' },
        @{ name='close-25m'; clearance=25.0; phase='close-inspection' },
        @{ name='retreat-1km'; clearance=1000.0; phase='retreat' },
        @{ name='reversal-5km'; clearance=5000.0; phase='reversal' }
    )
    if ($RouteProfile -eq 'FarOrbit') {
        $route = @(
            @{ name='far-orbit-10000km'; clearance=10000000.0; phase='far-orbit' },
            @{ name='far-orbit-100000km'; clearance=100000000.0; phase='far-orbit' }
        )
    } elseif ($RouteProfile -eq '100km') {
        $route = @(@{ name='approach-100km'; clearance=100000.0; phase='approach' })
    } elseif ($RouteProfile -eq 'Close') {
        $route = @(
            @{ name='approach-5km'; clearance=5000.0; phase='approach' },
            @{ name='approach-1km'; clearance=1000.0; phase='approach' },
            @{ name='close-25m'; clearance=25.0; phase='close-inspection' },
            @{ name='retreat-1km'; clearance=1000.0; phase='retreat' },
            @{ name='reversal-5km'; clearance=5000.0; phase='reversal' }
        )
    }
    if ($SkipCheckpoints -gt 0) { $route = @($route | Select-Object -Skip $SkipCheckpoints) }
    if ($LimitCheckpoints -gt 0) { $route = @($route | Select-Object -First $LimitCheckpoints) }
    foreach ($step in $route) { Invoke-Checkpoint $step.name $step.clearance $step.phase }
    if ($RouteProfile -in @('Sweep','Close') -and $LimitCheckpoints -eq 0) {
        $rapidOrigin = Get-Observation
        $rapidStartFrame = [long]$rapidOrigin.snapshot.general.frame_number
        $rapidFrameMap = @{}
        $rapidPerfMap = @{}
        foreach ($rapid in @(@{name='rapid-approach-100km';meters=100000.0},@{name='rapid-approach-1km';meters=1000.0},@{name='rapid-retreat-100km';meters=100000.0},@{name='rapid-reversal-5km';meters=5000.0})) {
            Send-Action @{ action='clearance'; meters=$rapid.meters } $rapid.name
            $rapidObs = Get-Observation
            $rapidFrame = [long]$rapidObs.snapshot.general.frame_number
            $rapidPerf = Get-PerformancePoint $rapidObs 'rapid-action' 0
            if ($rapidFrame -gt $rapidStartFrame) { $rapidPerfMap[[string]$rapidFrame] = $rapidPerf }
            foreach ($frameSample in @(Get-FrameSamples $rapidObs.resident)) { if ($null -ne $frameSample.frame -and [long]$frameSample.frame -gt $rapidStartFrame) { $rapidFrameMap[[string]$frameSample.frame] = $frameSample } }
        }
        $rapidFrames = @($rapidFrameMap.Values | Sort-Object { [long]$_.frame })
        $script:summary.rapid_action_frames = $rapidFrames
        $script:summary.rapid_action_performance_samples = @($rapidPerfMap.Values | Sort-Object { [long]$_.frame })
        Renew-Control
        $script:summary.rapid_action_distributions = Get-Distributions $(if ($rapidFrames.Count) { $rapidFrames } else { $script:summary.rapid_action_performance_samples })
        $script:summary.rapid_action_frame_range = @($rapidStartFrame, $(if ($rapidPerfMap.Count) { [long](($rapidPerfMap.Values | Measure-Object frame -Maximum).Maximum) } else { $rapidStartFrame }))
        Invoke-Checkpoint 'rapid-reversal-5km' 5000.0 'rapid-reversal'
        $regionOrigin = Get-Observation
        $effectiveSpeed = Get-Number $regionOrigin.snapshot.camera.navigation @('effective_speed_m_s')
        $travelSeconds = if ($null -ne $effectiveSpeed -and $effectiveSpeed -gt 0) { [Math]::Min(30.0, [Math]::Max(1.0, 25000.0 / $effectiveSpeed * 1.3)) } else { 20.0 }
        Send-Action @{ action='navigation'; translation=@(0.0,0.0,1.0); duration_s=$travelSeconds } 'change-Moon-region'
        $motionStartFrame = [long]$regionOrigin.snapshot.general.frame_number
        $motionFrameMap = @{}
        $motionPerfMap = @{}
        $motionWatch = [Diagnostics.Stopwatch]::StartNew()
        $regionDestination = $regionOrigin
        while ($motionWatch.Elapsed.TotalSeconds -lt [Math]::Min(30.0, $travelSeconds + 0.5)) {
            $regionDestination = Get-Observation
            $motionFrame = [long]$regionDestination.snapshot.general.frame_number
            $motionPerf = Get-PerformancePoint $regionDestination 'route-motion' $motionWatch.Elapsed.TotalSeconds
            if ($motionFrame -gt $motionStartFrame) { $motionPerfMap[[string]$motionFrame] = $motionPerf }
            foreach ($frameSample in @(Get-FrameSamples $regionDestination.resident)) { if ($null -ne $frameSample.frame -and [long]$frameSample.frame -gt $motionStartFrame) { $motionFrameMap[[string]$frameSample.frame] = $frameSample } }
            Renew-Control
            Start-Sleep -Seconds ([Math]::Min($PollSeconds, [Math]::Max(0.1, $travelSeconds + 0.5 - $motionWatch.Elapsed.TotalSeconds)))
        }
        $motionFrames = @($motionFrameMap.Values | Sort-Object { [long]$_.frame })
        $summary.route_motion_frames = $motionFrames
        $summary.route_motion_performance_samples = @($motionPerfMap.Values | Sort-Object { [long]$_.frame })
        Renew-Control
        $summary.route_motion_distributions = Get-Distributions $(if ($motionFrames.Count) { $motionFrames } else { $summary.route_motion_performance_samples })
        $summary.route_motion_frame_range = @($motionStartFrame, $(if ($motionPerfMap.Count) { [long](($motionPerfMap.Values | Measure-Object frame -Maximum).Maximum) } else { $motionStartFrame }))
        $from = @($regionOrigin.snapshot.camera.position_m); $to = @($regionDestination.snapshot.camera.position_m)
        $regionDistance = [Math]::Sqrt([Math]::Pow($from[0]-$to[0],2)+[Math]::Pow($from[1]-$to[1],2)+[Math]::Pow($from[2]-$to[2],2))
        $summary.route_actions += [pscustomobject]@{ name='region-change-displacement'; distance_m=$regionDistance; threshold_m=20000.0; requested_travel_seconds=$travelSeconds; effective_speed_m_s=$effectiveSpeed; passed=($regionDistance -ge 20000.0); origin_frame=$regionOrigin.snapshot.general.frame_number; destination_frame=$regionDestination.snapshot.general.frame_number }
        if ($regionDistance -lt 20000.0) { throw "Surface navigation moved only ${regionDistance}m; another-region acceptance needs at least 20000m." }
        Invoke-Checkpoint 'another-region-1km' 1000.0 'region-change'
    }
    $missingScheduledCaptures = @($summary.checkpoints | ForEach-Object { $_.scheduled_captures } | Where-Object { $_.status -ne 'captured' })
    $summary.status = if ($missingScheduledCaptures.Count -gt 0) { 'partial' } else { 'completed' }
} catch {
    $summary.status = 'failed'
    $summary.failures += [pscustomobject]@{ time_utc=[DateTime]::UtcNow.ToString('o'); message=$_.Exception.Message; script_stack=$_.ScriptStackTrace }
    throw
} finally {
    if ($null -ne $lease -and $null -ne $script:sessionId) {
        try { $null = Invoke-Dev @('control','release',$lease) } catch { $summary.failures += [pscustomobject]@{ time_utc=[DateTime]::UtcNow.ToString('o'); message="lease_release: $($_.Exception.Message)" } }
    }
    if ($null -ne $nativeProcess) {
        try {
            if (-not $nativeProcess.HasExited) {
                $null = $nativeProcess.CloseMainWindow()
                if (-not $nativeProcess.WaitForExit(15000)) { Stop-Process -Id $nativeProcess.Id -Force; $nativeProcess.WaitForExit() }
            }
            $summary.session_process_exit_code = $nativeProcess.ExitCode
        } catch { $summary.failures += [pscustomobject]@{ time_utc=[DateTime]::UtcNow.ToString('o'); message="owned_process_cleanup: $($_.Exception.Message)" } }
    }
    $summary.finished_utc = [DateTime]::UtcNow.ToString('o')
    if ($null -ne $script:outputPath -and (Test-Path -LiteralPath $script:outputPath)) {
        try {
            if ($summary.status -eq 'failed') {
                $failureDirectory = Join-Path $script:outputPath 'failure-artifacts'
                New-Item -ItemType Directory -Path $failureDirectory -Force | Out-Null
                if ($null -ne $script:lastObservation) { Write-JsonFile (Join-Path $failureDirectory 'latest-observation.json') $script:lastObservation }
                if ($null -ne $script:recentObservations -and $script:recentObservations.Count -gt 0) { Write-JsonFile (Join-Path $failureDirectory 'rolling-observations.json') @($script:recentObservations | Select-Object -Last 16) }
                if ($null -ne $script:activeStageSeries -and $script:activeStageSeries.completed -eq $false) { Write-JsonFile (Join-Path $failureDirectory 'active-stage-series.json') $script:activeStageSeries }
                Write-JsonFile (Join-Path $failureDirectory 'completed-stage-series.json') $summary.checkpoints
                Write-JsonFile (Join-Path $failureDirectory 'route-actions.json') $summary.route_actions
            }
            $finalFingerprints = Get-Fingerprints
            $summary.source_changed_during_run = $finalFingerprints.source_sha256 -ne $summary.fingerprints.source_sha256
            $summary.final_source_sha256 = $finalFingerprints.source_sha256
            $summary.final_runner_script_sha256 = $finalFingerprints.runner_script_sha256
            $summary.runner_script_changed_during_run = $finalFingerprints.runner_script_sha256 -ne $summary.fingerprints.runner_script_sha256
            $summary.final_git_status = $finalFingerprints.git_status
            Write-JsonFile (Join-Path $script:outputPath 'summary.json') $summary
        } catch { $summary.failures += [pscustomobject]@{ time_utc=[DateTime]::UtcNow.ToString('o'); message="final_fingerprint: $($_.Exception.Message)" }; try { Write-JsonFile (Join-Path $script:outputPath 'summary.json') $summary } catch { Write-Warning "Could not write summary.json: $_" } }
    }
    Pop-Location
}
if ($summary.status -notin @('completed','partial')) { exit 1 }
Write-Output "Native Slice 2D evidence: $script:outputPath"
if ($summary.status -eq 'partial') { exit 1 }
