#Requires -Version 5.1
<#
.SYNOPSIS
Run independent read-only Luna tasks concurrently through the existing ChatGPT login.
.EXAMPLE
./scripts/codex-parallel.ps1 -Task 'List workspace crates', 'Identify validation modes' -WhatIf
.NOTES
Run as a script, not dot-sourced. Nonzero child exits, timeouts and missing final
responses fail the run. This helper never launches editing workers.
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [ValidateCount(1, 6)]
    [ValidateNotNullOrEmpty()]
    [string[]] $Task,

    [ValidateSet('luna-explore', 'luna-review', 'luna-tests')]
    [string] $Agent = 'luna-explore',

    [ValidateRange(1, 60)]
    [int] $TimeoutMinutes = 10
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$model = 'gpt-6-luna'
$utf8 = [Text.UTF8Encoding]::new($false)
foreach ($taskText in $Task) {
    if ([string]::IsNullOrWhiteSpace($taskText)) { throw 'Tasks must not be blank.' }
}
if (-not $PSCmdlet.ShouldProcess($repo, "Start $($Task.Count) concurrent read-only $Agent tasks using $model (ChatGPT)")) {
    for ($n = 0; $n -lt $Task.Count; $n++) { Write-Output "Task $($n + 1): $($Task[$n])" }
    return
}

$cli = (Get-Command codex -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
if ([IO.Path]::GetExtension($cli) -ne '.exe') {
    throw 'This Windows launcher requires codex.exe on PATH, rather than a shell shim.'
}
# This helper reads only the literal instructions in the checked-in role format.
# Mandatory model/auth/sandbox settings below do not depend on parsing a role layer.
$roleFile = Join-Path $repo ('.codex/agents/' + $Agent + '.toml')
$roleText = [IO.File]::ReadAllText($roleFile)
$instructionMatches = [regex]::Matches($roleText, "(?ms)^developer_instructions = '''\r?\n(.*?)^'''\s*$")
if ($instructionMatches.Count -ne 1 -or $roleText -notmatch '(?m)^model = "gpt-6-luna"\s*$') {
    throw 'The selected role must use the checked-in literal instruction format and the intended Luna model.'
}
$instructions = $instructionMatches[0].Groups[1].Value

function Start-CodexProcess {
    param([string[]] $Arguments, [bool] $RedirectInput = $false)
    # Quote native argv for Windows without invoking a shell or evaluating task text.
    $quoted = foreach ($argument in $Arguments) {
        $escaped = [regex]::Replace($argument, '(\\*)"', '$1$1\"')
        '"' + [regex]::Replace($escaped, '(\\+)$', '$1$1') + '"'
    }
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $cli
    $info.Arguments = $quoted -join ' '
    $info.WorkingDirectory = $repo
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardInput = $RedirectInput
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = $utf8
    $info.StandardErrorEncoding = $utf8
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw 'Codex process did not start.' }
        return $process
    } catch {
        $process.Dispose()
        throw
    }
}

function Stop-OwnedProcess {
    param([Diagnostics.Process] $Process)
    if (-not $Process.HasExited) {
        # Stop only this owned process tree, including any active command child.
        & "$env:SystemRoot/System32/taskkill.exe" /PID $Process.Id /T /F 1>$null 2>$null
        if (-not $Process.WaitForExit(5000)) { throw 'Could not stop an owned Codex process tree.' }
    }
}

# Public CLI status only: never read/export auth.json, tokens or account identifiers.
$auth = Start-CodexProcess -Arguments @('-c', 'model_provider="openai"', '-c', 'forced_login_method="chatgpt"', 'login', 'status')
try {
    $authOutput = $auth.StandardOutput.ReadToEndAsync()
    $authErrors = $auth.StandardError.ReadToEndAsync()
    if (-not $auth.WaitForExit(30000)) { throw 'ChatGPT login verification timed out.' }
    $statusText = $authOutput.GetAwaiter().GetResult() + $authErrors.GetAwaiter().GetResult()
    if ($auth.ExitCode -ne 0 -or $statusText -notmatch '(?m)^Logged in using ChatGPT\s*$') {
        throw 'Cannot verify a ChatGPT login. Stop; do not substitute API billing or another provider.'
    }
} finally {
    try { Stop-OwnedProcess $auth } finally { $auth.Dispose() }
}

$runDirectory = Join-Path $repo ('.codex/results/' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $runDirectory
$children = @()
$results = @()
$launcherErrors = @()
try {
    # All children start before waiting. No shared session or inherited provider,
    # plugin, approval-rule or hook configuration; auth still uses existing Codex home.
    for ($n = 0; $n -lt $Task.Count; $n++) {
        $prefix = Join-Path $runDirectory ('task-{0:D2}' -f ($n + 1))
        $prompt = "Perform only this assigned task. Remain read-only; no Git mutations, edits or nested delegation. " +
            "The launcher enforces read-only even for luna-tests: report checks requiring writes as blocked. Return concise evidence.`n`n" + $Task[$n]
        [IO.File]::WriteAllText($prefix + '.prompt.txt', $prompt, $utf8)
        $arguments = @('exec', '--strict-config', '--ignore-user-config', '--ignore-rules', '--ephemeral',
            '--model', $model, '--sandbox', 'read-only', '--json', '--color', 'never',
            '--output-last-message', ($prefix + '.final.txt'),
            '-c', 'model_provider="openai"', '-c', 'forced_login_method="chatgpt"',
            '-c', 'approval_policy="never"', '-c', 'agents.enabled=false',
            '-c', 'model_reasoning_effort="high"', '-c', 'windows.sandbox="elevated"',
            '-c', ('developer_instructions=' + (ConvertTo-Json -InputObject $instructions -Compress)), '-')
        $process = Start-CodexProcess -Arguments $arguments -RedirectInput $true
        $child = [pscustomobject]@{
            Number = $n + 1; Process = $process; Prefix = $prefix
            Started = [DateTime]::UtcNow; Finished = $false; TimedOut = $false
            Output = $process.StandardOutput.ReadToEndAsync()
            Errors = $process.StandardError.ReadToEndAsync()
        }
        $children += $child
        $bytes = $utf8.GetBytes($prompt)
        $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
        $process.StandardInput.Close()
    }
    while (@($children | Where-Object { -not $_.Finished }).Count -gt 0) {
        foreach ($child in $children) {
            if ($child.Finished) { continue }
            $process = $child.Process
            if (([DateTime]::UtcNow - $child.Started).TotalMinutes -ge $TimeoutMinutes) {
                $child.TimedOut = $true
                Stop-OwnedProcess $process
            }
            $captureComplete = $child.Output.Status -eq 'RanToCompletion' -and $child.Errors.Status -eq 'RanToCompletion'
            if (-not $process.HasExited -or (-not $captureComplete -and -not $child.TimedOut)) { continue }
            $outputText = if ($child.Output.Status -eq 'RanToCompletion') { $child.Output.GetAwaiter().GetResult() } else { '' }
            $errorText = if ($child.Errors.Status -eq 'RanToCompletion') { $child.Errors.GetAwaiter().GetResult() } else { 'Stream capture did not complete before timeout.' }
            [IO.File]::WriteAllText($child.Prefix + '.jsonl', $outputText, $utf8)
            [IO.File]::WriteAllText($child.Prefix + '.stderr.txt', $errorText, $utf8)
            $finalPath = $child.Prefix + '.final.txt'
            $hasFinal = (Test-Path -LiteralPath $finalPath) -and
                -not [string]::IsNullOrWhiteSpace([IO.File]::ReadAllText($finalPath))
            $results += [pscustomobject]@{
                Task = $child.Number; Agent = $Agent; Model = $model; ExitCode = $process.ExitCode
                TimedOut = $child.TimedOut; HasFinalResponse = $hasFinal; CaptureComplete = $captureComplete
                StartedUtc = $child.Started.ToString('o'); FinishedUtc = [DateTime]::UtcNow.ToString('o')
                Output = $child.Prefix + '.jsonl'; Errors = $child.Prefix + '.stderr.txt'; Final = $finalPath
            }
            $child.Finished = $true
        }
        if (@($children | Where-Object { -not $_.Finished }).Count -gt 0) { Start-Sleep -Milliseconds 200 }
    }
} catch {
    $launcherErrors += $_.Exception.Message
} finally {
    try {
        foreach ($child in $children) {
            try { Stop-OwnedProcess $child.Process } catch { $launcherErrors += "Task $($child.Number) cleanup: $($_.Exception.Message)" }
            if (-not $child.Finished) {
                $results += [pscustomobject]@{
                    Task = $child.Number; Agent = $Agent; Model = $model; ExitCode = $null
                    TimedOut = $child.TimedOut; HasFinalResponse = $false; CaptureComplete = $false
                    StartedUtc = $child.Started.ToString('o'); FinishedUtc = [DateTime]::UtcNow.ToString('o')
                    Output = $child.Prefix + '.jsonl'; Errors = $child.Prefix + '.stderr.txt'; Final = $child.Prefix + '.final.txt'
                }
            }
            try { $child.Process.Dispose() } catch { $launcherErrors += "Task $($child.Number) disposal: $($_.Exception.Message)" }
        }
    } finally {
        [IO.File]::WriteAllText((Join-Path $runDirectory 'summary.json'), (ConvertTo-Json -InputObject @($results | Sort-Object Task) -Depth 4), $utf8)
        if ($launcherErrors.Count -gt 0) {
            [IO.File]::WriteAllText((Join-Path $runDirectory 'launcher-errors.txt'), ($launcherErrors -join [Environment]::NewLine), $utf8)
        }
        Write-Output "Task prompts, JSON output, stderr, final responses and summary: $runDirectory"
    }
}
$results | Sort-Object Task | Format-Table Task, ExitCode, TimedOut, HasFinalResponse, StartedUtc, FinishedUtc -AutoSize
if ($launcherErrors.Count -gt 0 -or @($results | Where-Object {
    $null -eq $_.ExitCode -or $_.ExitCode -ne 0 -or $_.TimedOut -or -not $_.HasFinalResponse -or -not $_.CaptureComplete
}).Count -gt 0) { exit 1 }
