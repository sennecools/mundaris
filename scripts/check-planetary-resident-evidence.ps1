#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string] $Directory,
    [ValidateRange(1,1000)][double] $MaximumNativeFrameMs = 100
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Directory).Path
$summaryPath = Join-Path $root 'summary.json'
if (-not (Test-Path -LiteralPath $summaryPath -PathType Leaf)) { throw "Missing summary.json in $root" }
$summary = Get-Content -LiteralPath $summaryPath -Raw | ConvertFrom-Json
$expected = @('approach-1000km','approach-500km','approach-250km','approach-100km','approach-25km','approach-5km','approach-1km','close-25m','retreat-1km','reversal-5km','rapid-reversal-5km','another-region-1km')
$seen = @($summary.checkpoints | Select-Object -ExpandProperty name)
$missing = @($expected | Where-Object { $_ -notin $seen })
$captureFailures = @()
$backendFailures = @()
$convergenceFailures = @()
$warmFailures = @()
$stallFailures = @()
$admissionFailures = @()
$usefulDetailFailures = @()
$pipelineFailures = @()
$newMetricRun = $null -ne $summary.evidence_metrics_schema -and [int]$summary.evidence_metrics_schema -ge 2
$regionChanges = @($summary.route_actions | Where-Object { $_.name -eq 'region-change-displacement' })
$legacyCheckpoints = @($summary.checkpoints | Where-Object { $_.expected_backend -eq 'LEGACY CPU MESH' })
$residentCheckpoints = @($summary.checkpoints | Where-Object { $_.expected_backend -eq 'RESIDENT TILE' })
function Test-FrameDistribution($Distribution, [string] $Label, [bool] $RequireInterval, [bool] $RequireAdmission) {
    if ($null -eq $Distribution) { $script:stallFailures += "${Label}: frame distribution unavailable"; return }
    foreach ($field in @('frame_cpu_ms','host_frame_ms')) {
        $maximum = $Distribution.$field.max
        if ($null -eq $maximum) { $script:stallFailures += "${Label}: $field maximum unavailable" }
        elseif ([double]$maximum -ge $script:MaximumNativeFrameMs) { $script:stallFailures += "${Label}: $field maximum ${maximum}ms meets/exceeds $($script:MaximumNativeFrameMs)ms bound" }
    }
    if ($RequireInterval) {
        $maximum = $Distribution.interval_ms.max
        if ($null -eq $maximum) { $script:stallFailures += "${Label}: native interval maximum unavailable" }
        elseif ([double]$maximum -ge $script:MaximumNativeFrameMs) { $script:stallFailures += "${Label}: native interval maximum ${maximum}ms meets/exceeds $($script:MaximumNativeFrameMs)ms bound" }
    }
    if ($RequireAdmission) {
        $admission = $Distribution.publication_admission_ms.max
        if ($null -eq $admission) { $script:admissionFailures += "${Label}: publication admission maximum unavailable" }
        elseif ([double]$admission -gt 2.0) { $script:admissionFailures += "${Label}: publication admission maximum ${admission}ms exceeds 2ms" }
        $overruns = $Distribution.publication_budget_overruns.max
        if ($null -eq $overruns) { $script:admissionFailures += "${Label}: publication budget overrun counter unavailable" }
        elseif ([double]$overruns -ne 0) { $script:admissionFailures += "${Label}: publication budget overrun count is $overruns" }
    }
}
foreach ($checkpoint in @($summary.checkpoints)) {
    if ($checkpoint.camera_transition_stopped -ne $true) { $convergenceFailures += "$($checkpoint.name): camera transition not stopped" }
    if ($checkpoint.useful_coverage_observed -ne $true) { $convergenceFailures += "$($checkpoint.name): useful drawable coverage not observed" }
    if ($checkpoint.settled_quality -ne $true) { $convergenceFailures += "$($checkpoint.name): settled quality absent (timeout=$($checkpoint.quality_timeout))" }
    if ($checkpoint.construction_idle_observed -ne $true) { $convergenceFailures += "$($checkpoint.name): settled construction idle state absent" }
    if ($checkpoint.backend_matches -ne $true) { $backendFailures += "$($checkpoint.name): backend '$($checkpoint.backend)' expected '$($checkpoint.expected_backend)'" }
    $capture = $checkpoint.capture
    $paths = @($capture.receipt.snapshot,$capture.receipt.viewport_image,$capture.receipt.full_image)
    if ($paths.Count -ne 3 -or @($paths | Where-Object { [string]::IsNullOrWhiteSpace([string]$_) -or -not (Test-Path -LiteralPath $_ -PathType Leaf) }).Count -gt 0) { $captureFailures += "$($checkpoint.name): native capture pair/files missing" }
    if (@($checkpoint.warm_samples).Count -lt 2) { $warmFailures += "$($checkpoint.name): warm interval has too few observations" }
    if ($checkpoint.expected_backend -eq 'RESIDENT TILE') {
        if ($null -eq $checkpoint.resident_planetary) { $warmFailures += "$($checkpoint.name): resident diagnostics unavailable"; continue }
        $warm = $checkpoint.warm_deltas
        if ($null -eq $warm.jobs_started -or $warm.jobs_started -ne 0) { $warmFailures += "$($checkpoint.name): tile generation delta is $($warm.jobs_started)" }
        if ($null -eq $warm.completed_generation_tiles -or $warm.completed_generation_tiles -ne 0) { $warmFailures += "$($checkpoint.name): completed generation delta is $($warm.completed_generation_tiles)" }
        foreach ($sample in @($checkpoint.warm_samples)) {
            if ($sample.construction_pending -ne $false) { $warmFailures += "$($checkpoint.name): construction pending or unavailable during warm interval"; break }
            foreach ($field in @('worker_queued','worker_running','completion_backlog')) {
                if ($null -eq $sample.$field -or [double]$sample.$field -ne 0) { $warmFailures += "$($checkpoint.name): $field is pending or unavailable during warm interval"; break }
            }
        }
        if ($null -eq $warm.sampled_tile_content_upload_bytes -or $warm.sampled_tile_content_upload_bytes -ne 0) { $warmFailures += "$($checkpoint.name): per-frame tile content upload sum is $($warm.sampled_tile_content_upload_bytes)" }
        if ($null -eq $warm.sampled_boundary_upload_bytes -or $warm.sampled_boundary_upload_bytes -ne 0) { $warmFailures += "$($checkpoint.name): per-frame boundary upload sum is $($warm.sampled_boundary_upload_bytes)" }
        if ($null -eq $warm.max_sampled_tile_upload_bytes_per_frame -or $warm.max_sampled_tile_upload_bytes_per_frame -ne 0) { $warmFailures += "$($checkpoint.name): sampled tile upload maximum is $($warm.max_sampled_tile_upload_bytes_per_frame)" }
        if ($null -eq $warm.max_sampled_boundary_upload_bytes_per_frame -or $warm.max_sampled_boundary_upload_bytes_per_frame -ne 0) { $warmFailures += "$($checkpoint.name): sampled boundary upload maximum is $($warm.max_sampled_boundary_upload_bytes_per_frame)" }
        if ($null -eq $warm.cumulative_tile_content_upload_bytes -or $warm.cumulative_tile_content_upload_bytes -ne 0) { $warmFailures += "$($checkpoint.name): cumulative tile upload delta is $($warm.cumulative_tile_content_upload_bytes)" }
        if ($null -eq $warm.cumulative_boundary_upload_bytes -or $warm.cumulative_boundary_upload_bytes -ne 0) { $warmFailures += "$($checkpoint.name): cumulative boundary upload delta is $($warm.cumulative_boundary_upload_bytes)" }
        if ($checkpoint.frame_sample_count -lt 1) { $warmFailures += "$($checkpoint.name): native warm frame sample window is empty" }
        if ($newMetricRun) {
            foreach ($window in @('convergence','warm')) {
                $pipelineSummary = $checkpoint."${window}_pipeline_summary"
                if ($null -eq $pipelineSummary -or $pipelineSummary.diagnostic_poll_count -lt 1) { $pipelineFailures += "$($checkpoint.name): $window pipeline diagnostic polls missing"; continue }
                if ($null -eq $pipelineSummary.max_publication_backlog_age_ms) { $pipelineFailures += "$($checkpoint.name): $window maximum backlog age missing" }
                foreach ($cause in @('blocked_by_split','blocked_by_merge','blocked_by_neighbor_dependency','blocked_by_residency_slot')) {
                    if ($null -eq $pipelineSummary.blocked_causes.$cause.maximum -or $null -eq $pipelineSummary.blocked_causes.$cause.positive_snapshot_count) { $pipelineFailures += "$($checkpoint.name): $window blocked-cause snapshots missing for $cause" }
                }
                foreach ($counter in @('completed_generation_tiles','boundary_preparation_completed_groups','publication_adopted_groups')) {
                    $rate = $pipelineSummary.cumulative_counter_rates.$counter
                    if ($null -eq $rate -or $null -eq $rate.delta -or $null -eq $rate.per_second) { $pipelineFailures += "$($checkpoint.name): $window cumulative $counter delta/rate missing" }
                }
                foreach ($throughput in @('generation_throughput_tiles_per_second','boundary_throughput_groups_per_second','publication_throughput_groups_per_second')) {
                    if ($null -eq $pipelineSummary.throughput_distributions.$throughput -or $pipelineSummary.throughput_distributions.$throughput.sample_count -lt 1) { $pipelineFailures += "$($checkpoint.name): $window $throughput distribution missing" }
                }
                if ($null -eq $pipelineSummary.diagnostic_poll_gap_ms -or $null -eq $pipelineSummary.diagnostic_poll_gap_ms.max) { $pipelineFailures += "$($checkpoint.name): $window diagnostic poll gap summary missing" }
            }
            if (@($checkpoint.convergence_samples | Where-Object { $null -eq $_.pipeline }).Count -gt 0 -or @($checkpoint.warm_samples | Where-Object { $null -eq $_.pipeline }).Count -gt 0) { $pipelineFailures += "$($checkpoint.name): one or more per-poll pipeline snapshots missing" }
        }
        if ($checkpoint.name -eq 'close-25m' -and $newMetricRun) {
            if ($null -eq $checkpoint.useful_detail_elapsed_ms -or [double]$checkpoint.useful_detail_elapsed_ms -ge 120000.0) { $usefulDetailFailures += "close-25m: useful-detail proxy arrival is missing or not below 120000ms ($($checkpoint.useful_detail_elapsed_ms))" }
            if ($checkpoint.useful_detail_proxy_error_certified -ne $false) { $usefulDetailFailures += 'close-25m: useful-detail proxy certification state must explicitly remain false' }
            if ([double]$checkpoint.useful_detail_proxy_error_threshold_px -ne 1.0) { $usefulDetailFailures += "close-25m: expected the current 1px proxy threshold, found $($checkpoint.useful_detail_proxy_error_threshold_px)" }
        }
    } elseif ($checkpoint.expected_backend -eq 'LEGACY CPU MESH') {
        if ($null -eq $checkpoint.warm_performance_distributions -or $checkpoint.warm_performance_distributions.sample_count -lt 1 -or $null -eq $checkpoint.warm_performance_distributions.frame_cpu_ms.max) { $warmFailures += "$($checkpoint.name): legacy CPU performance samples unavailable" }
        if ($null -eq $checkpoint.convergence_frame_distributions -or $checkpoint.convergence_frame_distributions.sample_count -lt 1 -or $null -eq $checkpoint.convergence_frame_distributions.frame_cpu_ms.max) { $warmFailures += "$($checkpoint.name): legacy convergence CPU samples unavailable" }
    }
    $resident = $checkpoint.expected_backend -eq 'RESIDENT TILE'
    Test-FrameDistribution $checkpoint.convergence_frame_distributions "$($checkpoint.name) convergence" $resident $resident
    if ($resident) { Test-FrameDistribution $checkpoint.frame_distributions "$($checkpoint.name) warm" $true $true }
    else { Test-FrameDistribution $checkpoint.warm_performance_distributions "$($checkpoint.name) warm CPU poll" $false $false }
}
if ($newMetricRun -and $legacyCheckpoints.Count -eq 0 -and $residentCheckpoints.Count -gt 0 -and @($residentCheckpoints | Where-Object { $_.name -eq 'close-25m' }).Count -eq 0) {
    $usefulDetailFailures += 'close-25m: checkpoint missing; useful-detail proxy arrival cannot be established'
}
if ($residentCheckpoints.Count -gt 0) {
    Test-FrameDistribution $summary.bootstrap_frame_distributions 'startup and Moon binding window' $true $true
    Test-FrameDistribution $summary.rapid_action_distributions 'rapid action window' $true $true
    Test-FrameDistribution $summary.route_motion_distributions 'route motion window' $true $true
}
$checks = [ordered]@{
    route_completed = $summary.status -eq 'completed'
    lifecycle_cleanup_clean = $null -eq $summary.failures -or @($summary.failures).Count -eq 0
    all_required_checkpoints_present = $missing.Count -eq 0
    another_surface_region_reached = $regionChanges.Count -eq 1 -and $regionChanges[0].passed -eq $true -and $regionChanges[0].distance_m -ge 20000.0
    camera_and_quality_converged = $convergenceFailures.Count -eq 0
    expected_backend_at_every_checkpoint = $backendFailures.Count -eq 0
    all_native_capture_pairs_present = $captureFailures.Count -eq 0
    resident_warm_zero_generation_and_uploads = if ($legacyCheckpoints.Count -gt 0) { 'N/A for legacy backend' } else { $warmFailures.Count -eq 0 }
    backend_appropriate_warm_evidence = $warmFailures.Count -eq 0
    legacy_poll_performance_available = $legacyCheckpoints.Count -eq 0 -or @($legacyCheckpoints | Where-Object { $_.warm_performance_distributions.sample_count -lt 1 -or $_.convergence_frame_distributions.sample_count -lt 1 -or $null -eq $_.warm_performance_distributions.frame_cpu_ms.max -or $null -eq $_.convergence_frame_distributions.frame_cpu_ms.max }).Count -eq 0
    native_frame_stall_bound = $stallFailures.Count -eq 0
    publication_admission_budget = $admissionFailures.Count -eq 0
    close_view_useful_detail_proxy_under_120s = if ($legacyCheckpoints.Count -gt 0 -or -not $newMetricRun) { 'N/A for legacy or historical evidence schema' } else { $usefulDetailFailures.Count -eq 0 }
    resident_pipeline_metrics_complete = if ($legacyCheckpoints.Count -gt 0 -or -not $newMetricRun) { 'N/A for legacy or historical evidence schema' } else { $pipelineFailures.Count -eq 0 }
    baseline_fingerprints_present = $null -ne $summary.fingerprints -and $null -ne $summary.fingerprints.head -and $null -ne $summary.fingerprints.source_sha256 -and $null -ne $summary.fingerprints.executable_sha256
    source_unchanged_during_run = $summary.source_changed_during_run -eq $false -and $summary.final_source_sha256 -eq $summary.fingerprints.source_sha256
}
$result = [ordered]@{
    scope = 'Native real-Moon Slice 2D acceptance route; each checkpoint includes paired image/JSON and a stationary warm interval.'
    status = if (@($checks.Values | Where-Object { -not $_ }).Count -eq 0) { 'PASS' } else { 'FAIL' }
    maximum_native_frame_ms = $MaximumNativeFrameMs
    checks = $checks
    missing_checkpoints = $missing
    convergence_failures = $convergenceFailures
    backend_failures = $backendFailures
    capture_failures = $captureFailures
    warm_failures = $warmFailures
    resident_checkpoint_count = $residentCheckpoints.Count
    legacy_checkpoint_count = $legacyCheckpoints.Count
    native_stall_failures = $stallFailures
    publication_admission_failures = $admissionFailures
    useful_detail_failures = $usefulDetailFailures
    pipeline_metric_failures = $pipelineFailures
    useful_detail_acceptance_scope = 'Approximate uncertified selector projected-error proxy only; does not establish visual acceptance.'
    historical_metric_schema = if ($newMetricRun) { 'current metrics schema enforced' } else { 'older evidence accepted without current pipeline and useful-detail metrics' }
    source_sha256 = $summary.fingerprints.source_sha256
    executable_sha256 = $summary.fingerprints.executable_sha256
    checkpoint_count = @($summary.checkpoints).Count
    evidence_directory = $root
}
$result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $root 'validation.json') -Encoding UTF8
$checks | ConvertTo-Json
if ($result.status -ne 'PASS') { exit 1 }
