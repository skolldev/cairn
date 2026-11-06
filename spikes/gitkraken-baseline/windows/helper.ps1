# Runs elevated so the driver in run.py doesn't have to, which would make GitKraken elevated too.
# Purges the file cache on cold.trigger and runs PresentMon on pm.trigger. stop.trigger ends it.
$ErrorActionPreference = 'Continue'
Set-Location $PSScriptRoot
Remove-Item cold.trigger, cold.done, pm.trigger, pm.started, pm.done, stop.trigger -ErrorAction SilentlyContinue
$res = Join-Path $PSScriptRoot '..\results\windows'
New-Item -ItemType Directory -Force $res | Out-Null
Get-MpPreference | Select-Object ExclusionPath, ExclusionProcess, ExclusionExtension | Format-List |
    Out-File -Encoding utf8 (Join-Path $res 'defender-exclusions.txt')
New-Item -ItemType File -Force helper.ready | Out-Null
Write-Host 'Ready. Leave this window open.'
while (-not (Test-Path stop.trigger)) {
    if (Test-Path cold.trigger) {
        Remove-Item cold.trigger
        .\target\release\probe.exe purge | Out-File -Encoding ascii cold.result
        New-Item -ItemType File -Force cold.done | Out-Null
        Write-Host "purged $(Get-Date -Format T)"
    }
    if (Test-Path pm.trigger) {
        # One line: process name, seconds, output file name.
        $p, $s, $o = (Get-Content pm.trigger -TotalCount 1).Trim() -split ' '
        Remove-Item pm.trigger
        if ($p -match '^[\w.]+$' -and $s -match '^\d+$' -and $o -match '^[\w.-]+$') {
            $args = @('--process_name', $p, '--output_file', (Join-Path $res $o), '--timed', $s,
                '--terminate_after_timed', '--stop_existing_session', '--no_console_stats',
                '--qpc_time_ms', '--session_name', 'cairn_pm')
            $proc = Start-Process .\bin\PresentMon.exe -ArgumentList $args -PassThru -WindowStyle Hidden
            Start-Sleep -Milliseconds 1500
            New-Item -ItemType File -Force pm.started | Out-Null
            $proc.WaitForExit()
            New-Item -ItemType File -Force pm.done | Out-Null
            Write-Host "presentmon $o exit $($proc.ExitCode)"
        }
    }
    Start-Sleep -Milliseconds 100
}
Remove-Item stop.trigger, helper.ready -ErrorAction SilentlyContinue
