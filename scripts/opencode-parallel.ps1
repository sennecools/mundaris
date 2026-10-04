#Requires -Version 5.1
<#
.SYNOPSIS
Run independent read-only Luna tasks concurrently using the existing ChatGPT connection.
.EXAMPLE
./scripts/opencode-parallel.ps1 -Task 'List workspace crates', 'Identify validation modes' -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [ValidateCount(1, 6)]
    [ValidateNotNullOrEmpty()]
    [string[]] $Task,

    [ValidateSet('luna-explore', 'luna-review', 'luna-tests')]
    [string] $Agent = 'luna-explore'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$model = 'openai/gpt-6-luna'
$cli = (Get-Command opencode -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
foreach ($text in $Task) {
    if ([string]::IsNullOrWhiteSpace($text)) { throw 'Tasks must not be blank.' }
}
if (-not $PSCmdlet.ShouldProcess($repo, "Start $($Task.Count) concurrent read-only $Agent tasks using $model (ChatGPT OAuth)")) {
    for ($n = 0; $n -lt $Task.Count; $n++) {
        Write-Output "Task $($n + 1): $($Task[$n])"
    }
    return
}

# Capture only non-secret public API metadata. Never use auth export or read credential files.
Push-Location $repo
try {
    $location = '?location[directory]=' + [uri]::EscapeDataString($repo)
    $provider = (& $cli api get ("/api/provider/openai" + $location) | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Provider inspection failed.' }
    $integration = (& $cli api get ("/api/integration/openai" + $location) | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Authentication metadata inspection failed.' }
    $connections = @($integration.data.connections)
    if ($provider.data.integrationID -ne 'openai' -or
        $provider.data.package -ne '@opencode/ai/providers/openai' -or
        $provider.data.activation -eq 'disabled' -or
        $provider.data.settings.baseURL -ne 'https://chatgpt.com/backend-api/codex' -or
        $connections.Count -ne 1 -or $connections[0].method -ne 'oauth') {
        throw 'Cannot verify the single ChatGPT OAuth connection. Stop; do not substitute API billing, Go, or OpenRouter.'
    }
    $agents = (& $cli debug agents | Out-String) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Agent discovery failed.' }
    $selected = @($agents | Where-Object id -eq $Agent)
    if ($selected.Count -ne 1 -or $selected[0].model.providerID -ne 'openai' -or
        $selected[0].model.id -ne 'gpt-6-luna') {
        throw 'The selected repository agent is missing or is not pinned to the intended Luna model.'
    }
    $editRules = @($selected[0].permissions | Where-Object {
        'edit' -like $_.action -and $_.resource -eq '*'
    })
    if ($editRules.Count -eq 0 -or $editRules[-1].effect -ne 'deny' -or
        @($selected[0].permissions | Where-Object {
            'edit' -like $_.action -and $_.effect -eq 'allow' -and $_.resource -ne '*'
        }).Count -gt 0) {
        throw 'The CLI fallback requires an agent whose effective edit permissions are denied.'
    }
} finally {
    Pop-Location
}

$runDirectory = Join-Path $repo ('.opencode/results/' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $runDirectory
$jobs = @()
$results = @()
try {
    # Start every process before waiting for any result. No shared sessions or editing agents.
    for ($n = 0; $n -lt $Task.Count; $n++) {
        $prefix = Join-Path $runDirectory ('task-{0:D2}' -f ($n + 1))
        $taskFile = $prefix + '.txt'
        [IO.File]::WriteAllText($taskFile, $Task[$n], [Text.UTF8Encoding]::new($false))
        $jobs += Start-Job -ArgumentList $cli, $repo, $model, $Agent, $taskFile, $prefix, ($n + 1) -ScriptBlock {
            param($cli, $repo, $model, $agent, $taskFile, $prefix, $number)
            Set-Location $repo
            $start = [DateTime]::UtcNow
            $code = 1
            try {
                & $cli run --model $model --agent $agent --format json --file $taskFile `
                    'Perform only the attached read-only task. No edits or Git mutations. Do not delegate. Return concise evidence.' `
                    1> ($prefix + '.jsonl') 2> ($prefix + '.stderr.txt')
                $code = $LASTEXITCODE
            } catch {
                $_.Exception.Message | Out-File ($prefix + '.stderr.txt') -Append
            }
            [pscustomobject]@{
                Task = $number
                ExitCode = $code
                StartedUtc = $start.ToString('o')
                FinishedUtc = [DateTime]::UtcNow.ToString('o')
                Output = $prefix + '.jsonl'
                Errors = $prefix + '.stderr.txt'
            }
        }
    }
    foreach ($job in $jobs) {
        $results += Receive-Job -Job $job -Wait -ErrorAction Stop
        if ($job.State -ne 'Completed') { throw 'A parallel task job failed.' }
    }
    $results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $runDirectory 'summary.json')
    $results | Format-Table Task, ExitCode, StartedUtc, FinishedUtc -AutoSize
    Write-Output "Separate task prompts, JSON output, stderr, and summary: $runDirectory"
    if (@($results | Where-Object ExitCode -ne 0).Count -gt 0) { exit 1 }
} finally {
    foreach ($job in $jobs) {
        if ($job.State -eq 'Running') { Stop-Job -Job $job }
        Remove-Job -Job $job -Force
    }
}
