#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string] $Directory)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Directory).Path
$records = @()
foreach ($file in Get-ChildItem -LiteralPath $root -Filter '*.json') {
    $snapshot = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json
    if ($snapshot -is [System.Array]) { continue }
    if ($null -eq $snapshot.resident_tile -or $null -eq $snapshot.capture) { continue }
    if ($snapshot.capture.image -isnot [string] -or [string]::IsNullOrWhiteSpace($snapshot.capture.image)) { continue }
    $tile = $snapshot.resident_tile
    $image = Join-Path $root $snapshot.capture.image
    $records += [pscustomobject]@{
        name = $file.BaseName
        snapshot_sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        paired_image_exists = Test-Path -LiteralPath $image -PathType Leaf
        family = $tile.identity.family
        identity = $tile.identity
        radius_m = $tile.identity.radius_m
        address = $tile.address
        build_ms = $tile.builder.cpu_build_ms
        world_to_tile = $tile.world_to_tile
        tile_to_gpu = $tile.tile_to_gpu
        gpu_residency = $tile.gpu_residency
        performance = $snapshot.performance
        adapter = $snapshot.capture.adapter
        backend = $snapshot.capture.backend
    }
}
$cold = $records | Where-Object name -EQ 'cold'
$warm = $records | Where-Object name -EQ 'warm_120'
$presentation = @($records | Where-Object { $_.name -in @('camera_light','height','normals','material','uv','grid') })
$precision = @($records | Where-Object { $_.name -like 'precision_*' })
$failedReconstruction = @($records | Where-Object { $null -eq $_.tile_to_gpu -or -not $_.tile_to_gpu.passed })
$checks = [ordered]@{
    all_images_paired = @($records | Where-Object { -not $_.paired_image_exists }).Count -eq 0
    canonical_cold_upload = $null -ne $cold -and $cold.gpu_residency.tile_content_upload_count -eq 1
    canonical_definition_identity = $null -ne $cold -and $cold.identity.definition_words[1] -eq 5931033225171238913
    warm_120_no_upload = $null -ne $warm -and $warm.gpu_residency.tile_content_upload_bytes -eq 0 -and $warm.gpu_residency.cumulative_content_upload_count -eq $cold.gpu_residency.cumulative_content_upload_count
    presentation_reuses_content = $presentation.Count -eq 6 -and @($presentation | Where-Object { $_.gpu_residency.tile_content_upload_bytes -ne 0 -or $_.gpu_residency.cumulative_content_upload_count -ne $cold.gpu_residency.cumulative_content_upload_count }).Count -eq 0
    revision_invalidates_once = @($records | Where-Object name -EQ 'invalidation_revision')[0].gpu_residency.cumulative_content_upload_count -eq ($cold.gpu_residency.cumulative_content_upload_count + 1)
    all_gpu_reconstruction_pass = $records.Count -gt 0 -and $failedReconstruction.Count -eq 0
    precision_matrix_complete = $precision.Count -eq 54
    generic_families_present = @($records | Where-Object { $_.name -like 'family_*' } | Select-Object -ExpandProperty family -Unique).Count -eq 3
}
$result = [ordered]@{
    scope = 'Focused offscreen resident-tile evidence; source review, historical preservation, focused tests and native gates are separate.'
    checks = $checks
    passed = @($checks.Values | Where-Object { -not $_ }).Count -eq 0
    failed_reconstruction = @($failedReconstruction | Select-Object name,tile_to_gpu)
    records = $records
}
$result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $root 'evidence-analysis.json') -Encoding UTF8
$checks | ConvertTo-Json
if (-not $result.passed) { exit 1 }
