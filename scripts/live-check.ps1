# Daily check that every source still works against the real site, run on this PC
# (the GitHub Actions twin is .github/workflows/live-sources.yml).
#
#   powershell -ExecutionPolicy Bypass -File scripts\live-check.ps1              run once
#   powershell -ExecutionPolicy Bypass -File scripts\live-check.ps1 -Install     run daily at 08:00 (Task Scheduler)
#   powershell -ExecutionPolicy Bypass -File scripts\live-check.ps1 -Uninstall   remove the daily task
#
# Logs go to %LOCALAPPDATA%\KumaDoujin\live-check\. When a source fails: a Windows notification, and an
# issue on GitHub (label live-check) if the GitHub CLI is logged in.
param(
    [switch]$Install,
    [switch]$Uninstall,
    [string]$At = "08:00"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$task = "KumaDoujin live check"

if ($Uninstall) {
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
    Write-Host "removed '$task'"
    exit 0
}
if ($Install) {
    $action = New-ScheduledTaskAction -Execute "powershell.exe" `
        -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSCommandPath`"" `
        -WorkingDirectory $root
    $trigger = New-ScheduledTaskTrigger -Daily -At $At
    # StartWhenAvailable: a run missed while the PC was off happens at the next start
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Hours 1)
    Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Settings $settings `
        -Description "Tests every KumaDoujin source against the real sites" -Force | Out-Null
    Write-Host "registered '$task' daily at $At"
    exit 0
}

Set-Location $root
$logDir = Join-Path $env:LOCALAPPDATA "KumaDoujin\live-check"
New-Item -ItemType Directory -Force $logDir | Out-Null
$log = Join-Path $logDir ("{0:yyyy-MM-dd_HHmm}.log" -f (Get-Date))
# keep the last 30 logs
Get-ChildItem $logDir -Filter *.log | Sort-Object Name -Descending | Select-Object -Skip 30 | Remove-Item

$ErrorActionPreference = "Continue"
cargo build --bin kuma-browser *>> $log
$env:KUMA_BROWSER_EXE = Join-Path $root "target\debug\kuma-browser.exe"
cargo test --test live_mangastream -- --ignored *>> $log
cargo test --test live_gated -- --ignored --test-threads=1 *>> $log

$text = Get-Content $log -Raw
$failed = ([regex]::Matches($text, '(?m)^test ([a-z_]+) \.\.\. FAILED') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique) -join ", "
$passed = ([regex]::Matches($text, '(?m)^test ([a-z_]+) \.\.\. ok')).Count
if (-not $failed -and $passed -eq 0) { $failed = "build or test run (see log)" }

if (-not $failed) {
    Add-Content $log "== all $passed sources ok"
    exit 0
}
Add-Content $log "== FAILED: $failed"

# Windows notification
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$tip = New-Object System.Windows.Forms.NotifyIcon
$icon = Join-Path $root "assets\icon.ico"
$tip.Icon = if (Test-Path $icon) { New-Object System.Drawing.Icon $icon } else { [System.Drawing.SystemIcons]::Warning }
$tip.Visible = $true
$tip.ShowBalloonTip(15000, "KumaDoujin: sources broke", "Failed: $failed`nLog: $log", [System.Windows.Forms.ToolTipIcon]::Warning)
Start-Sleep -Seconds 16
$tip.Dispose()

# GitHub issue (same label as the Actions workflow)
if (Get-Command gh -ErrorAction SilentlyContinue) {
    $body = "Daily live check on $env:COMPUTERNAME failed for: **$failed**`n`nLog: ``$log``"
    $existing = gh issue list --state open --label live-check --json number --jq '.[0].number' 2>$null
    if ($existing) {
        gh issue comment $existing --body $body | Out-Null
    } else {
        gh label create live-check --color B60205 --description "Daily live source check" 2>$null | Out-Null
        gh issue create --title "Live source check failed: $failed" --label live-check --body $body | Out-Null
    }
}
exit 1
