<#
.SYNOPSIS
  Runs a dev-only EdenExplorer UI probe script (see scripts/UI_PROBE.md).

.DESCRIPTION
  Does not build anything. Build the probe binary first:
    $env:CARGO_TARGET_DIR = "target/probe"; cargo build --release --features ui-probe

  Prepares a fresh isolated data directory (optionally seeded), starts the probe
  exe with EDEN_PROBE_SCRIPT / EDEN_PROBE_DATA_DIR set for that process only,
  waits for it to exit (killing only that PID after the timeout) and prints the
  probe log. Also reports the real cursor position, the foreground window and
  the probe window rectangle so that non-intrusiveness can be checked.

.EXAMPLE
  ./scripts/ui-probe.ps1 -Script scripts/probe/sidebar_separator.txt -Seed scripts/probe/seed_vertical_tabs
#>
param(
  [Parameter(Mandatory = $true)][string]$Script,
  [string]$DataDir = (Join-Path $env:TEMP "eden-probe-data"),
  [string]$Exe,
  [string]$Seed,
  [int]$TimeoutSec = 60
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class EdenProbeWin32 {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  public static string Cursor() { POINT p; GetCursorPos(out p); return p.X + "," + p.Y; }
  public static string Foreground() {
    IntPtr h = GetForegroundWindow();
    StringBuilder sb = new StringBuilder(512);
    GetWindowText(h, sb, sb.Capacity);
    return "0x" + h.ToString("X") + " '" + sb.ToString() + "'";
  }
  public static string ProbeRect() {
    IntPtr h = FindWindow(null, "EdenExplorer Probe");
    if (h == IntPtr.Zero) return null;
    RECT r; GetWindowRect(h, out r);
    return "(" + r.Left + "," + r.Top + ")-(" + r.Right + "," + r.Bottom + ")";
  }
}
"@

if (-not $Exe) {
  $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
  $Exe = Join-Path $scriptDir "..\target\probe\release\EdenExplorer.exe"
}
$scriptPath = (Resolve-Path $Script).Path
$exePath = (Resolve-Path $Exe).Path
$logPath = [System.IO.Path]::ChangeExtension($scriptPath, ".log")

# Fresh, isolated data dir (never the real %LOCALAPPDATA%\ExplorerEden).
$realDataDir = Join-Path $env:LOCALAPPDATA "ExplorerEden"
$DataDir = [System.IO.Path]::GetFullPath($DataDir)
if ($DataDir.TrimEnd('\') -ieq $realDataDir.TrimEnd('\')) {
  throw "Refusing to use the real data dir $realDataDir as probe data dir."
}
if (Test-Path $DataDir) { Remove-Item -Recurse -Force $DataDir }
New-Item -ItemType Directory -Force $DataDir | Out-Null
if ($Seed) {
  Copy-Item -Recurse -Force (Join-Path (Resolve-Path $Seed).Path "*") $DataDir
}
if (Test-Path $logPath) { Remove-Item -Force $logPath }

function Get-RealDataHashes {
  if (-not (Test-Path $realDataDir)) { return "" }
  $sha = [System.Security.Cryptography.SHA256]::Create()
  (Get-ChildItem -Recurse -File $realDataDir | Sort-Object FullName | ForEach-Object {
      $hash = [System.BitConverter]::ToString($sha.ComputeHash([System.IO.File]::ReadAllBytes($_.FullName)))
      "$($_.FullName)|$($_.LastWriteTimeUtc.Ticks)|$hash"
    }) -join "`n"
}

$hashesBefore = Get-RealDataHashes
$cursorBefore = [EdenProbeWin32]::Cursor()
$foregroundBefore = [EdenProbeWin32]::Foreground()

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exePath
$psi.WorkingDirectory = Split-Path $scriptPath
$psi.UseShellExecute = $false
$psi.EnvironmentVariables["EDEN_PROBE_SCRIPT"] = $scriptPath
$psi.EnvironmentVariables["EDEN_PROBE_DATA_DIR"] = $DataDir

Write-Host "probe exe : $exePath"
Write-Host "script    : $scriptPath"
Write-Host "data dir  : $DataDir"

$proc = [System.Diagnostics.Process]::Start($psi)
$foregroundSeen = @{}
$cursorSeen = @{}
$probeRect = $null
$deadline = (Get-Date).AddSeconds($TimeoutSec)
while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
  $foregroundSeen[[EdenProbeWin32]::Foreground()] = $true
  $cursorSeen[[EdenProbeWin32]::Cursor()] = $true
  $rect = [EdenProbeWin32]::ProbeRect()
  if ($rect) { $probeRect = $rect }
  Start-Sleep -Milliseconds 100
}
$timedOut = -not $proc.HasExited
if ($timedOut) {
  Write-Warning "Timeout after $TimeoutSec s, killing probe PID $($proc.Id)"
  Stop-Process -Id $proc.Id -Force
}

$cursorAfter = [EdenProbeWin32]::Cursor()
$foregroundAfter = [EdenProbeWin32]::Foreground()
$hashesAfter = Get-RealDataHashes

Write-Host ""
Write-Host "---- probe log ($logPath) ----"
if (Test-Path $logPath) { Get-Content $logPath } else { Write-Warning "no log written" }
Write-Host "------------------------------"
Write-Host "exit code        : $(if ($timedOut) { 'killed (timeout)' } else { $proc.ExitCode })"
Write-Host "probe window rect: $probeRect"
Write-Host "cursor           : before $cursorBefore, after $cursorAfter, distinct samples during run: $($cursorSeen.Keys -join ' | ')"
Write-Host "foreground       : before $foregroundBefore, after $foregroundAfter"
Write-Host "foreground seen  : $($foregroundSeen.Keys -join ' | ')"
Write-Host "real data dir    : $(if ($hashesBefore -eq $hashesAfter) { 'unchanged' } else { 'CHANGED!' }) ($realDataDir)"
if ($timedOut) { exit 1 }
