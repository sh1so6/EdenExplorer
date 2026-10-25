<#
.SYNOPSIS
  Regression harness: a WM_CLOSE (taskkill without /F) sent shortly after start
  must make EdenExplorer exit.

.DESCRIPTION
  Starts the probe exe (scripts/UI_PROBE.md) with a script that only waits,
  sends `taskkill /PID <pid>` (no /F, i.e. WM_CLOSE to the process windows)
  after each delay and checks that the process exits within -ExitTimeoutMs
  (FAIL otherwise). A process still alive then gets until -HangTimeoutMs:
  exiting by then is reported as SLOW (startup took long, e.g. a loaded
  machine), else as HANG and it is killed with /F (only PIDs started here).
  The probe script waits 60 s, so it never quits on its own in that time.
  Optionally seeds session.json and reports whether it survived/was written.

.EXAMPLE
  ./scripts/probe/early_close.ps1 -Runs 3
#>
param(
  [int[]]$DelaysMs = @(0, 50, 100, 200, 300, 450, 600, 900, 1200, 2000),
  [int]$Runs = 3,
  [int]$ExitTimeoutMs = 3000,
  [int]$HangTimeoutMs = 15000,
  [string]$Exe,
  [string]$Seed,
  [string]$DataDir = (Join-Path $env:TEMP "eden-probe-early-close")
)

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Exe) { $Exe = Join-Path $scriptDir "..\..\target\probe\release\EdenExplorer.exe" }
$Exe = (Resolve-Path $Exe).Path

$probeScript = Join-Path $env:TEMP "eden-probe-early-close.txt"
Set-Content -Path $probeScript -Value "wait_ms 60000" -Encoding ASCII

$fail = 0
$hang = 0
$total = 0
foreach ($delay in $DelaysMs) {
  for ($run = 1; $run -le $Runs; $run++) {
    if (Test-Path $DataDir) { Remove-Item -Recurse -Force $DataDir }
    New-Item -ItemType Directory -Path $DataDir | Out-Null
    if ($Seed) { Copy-Item -Path (Join-Path $Seed "*") -Destination $DataDir -Recurse }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.UseShellExecute = $false
    $psi.EnvironmentVariables["EDEN_PROBE_SCRIPT"] = $probeScript
    $psi.EnvironmentVariables["EDEN_PROBE_DATA_DIR"] = $DataDir
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = [System.Diagnostics.Process]::Start($psi)
    Start-Sleep -Milliseconds $delay
    $sentAt = $sw.ElapsedMilliseconds
    $tk = & taskkill.exe /PID $p.Id 2>&1
    $exited = $p.WaitForExit($ExitTimeoutMs)
    $exitMs = $sw.ElapsedMilliseconds - $sentAt
    $total++
    # session.json after the exit: without a seed it must exist (written by the
    # exit flush); with a seeded session.json its tabs must be unchanged.
    $sessionPath = Join-Path $DataDir "session.json"
    $session = if (-not (Test-Path $sessionPath)) {
      "no-session"
    } elseif ($Seed -and (Test-Path (Join-Path $Seed "session.json"))) {
      $want = Get-Content (Join-Path $Seed "session.json") -Raw | ConvertFrom-Json | ConvertTo-Json -Depth 5 -Compress
      $got = Get-Content $sessionPath -Raw | ConvertFrom-Json | ConvertTo-Json -Depth 5 -Compress
      if ($want -eq $got) { "session.json same as seed" } else { "session.json CHANGED: $got" }
    } else {
      "session.json"
    }
    if ($exited -and ($session -eq "no-session" -or $session -like "*CHANGED*")) {
      $fail++
      "delay={0,5}ms run={1} FAIL exit after {2} ms (code {3}) {4}" -f $delay, $run, $exitMs, $p.ExitCode, $session
    } elseif ($exited) {
      "delay={0,5}ms run={1} PASS exit after {2} ms (code {3}) {4}" -f $delay, $run, $exitMs, $p.ExitCode, $session
    } else {
      $fail++
      $states = try {
        ($p.Threads | Select-Object -First 1 | ForEach-Object { "$($_.Id):$($_.ThreadState)/$($_.WaitReason)" }) -join " "
      } catch { "?" }
      if ($p.WaitForExit([Math]::Max(0, $HangTimeoutMs - $ExitTimeoutMs))) {
        "delay={0,5}ms run={1} FAIL (SLOW) exit after {2} ms (code {3}); main thread at {4} ms: {5}" -f $delay, $run, ($sw.ElapsedMilliseconds - $sentAt), $p.ExitCode, $ExitTimeoutMs, $states
      } else {
        $hang++
        "delay={0,5}ms run={1} FAIL (HANG) still alive after {2} ms; taskkill: {3}" -f $delay, $run, $HangTimeoutMs, ($tk -join " ")
        & taskkill.exe /F /PID $p.Id | Out-Null
        $p.WaitForExit(5000) | Out-Null
      }
    }
  }
}
"{0}/{1} failed ({2} hung)" -f $fail, $total, $hang
