# Takes screenshots of Pulse with environment hooks, one start per case.
# Sends no keys, makes no clicks, never takes the focus, never touches the
# clipboard, and stops only the processes it started (by the recorded PID).
#
# -CasesFile is a JSON file: [{"name": "mint-light", "env": {"PULSE_THEME": "light", ...}}, ...]
# Every start gets its own PULSE_SETTINGS_DIR and PULSE_INSTANCE_NAME, PULSE_SHOW=1
# and the exe is started under a copy name so it is never mistaken for a real Pulse.
# Writes <name>.png and shots.json (window rectangle, dpi, work area, gaps, styles).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\shots.ps1 `
#     -Exe <pulse-native.exe> -Out <folder> -CasesFile <cases.json>
param(
  [Parameter(Mandatory)] [string]$Exe,
  [Parameter(Mandatory)] [string]$Out,
  [Parameter(Mandatory)] [string]$CasesFile,
  [int]$SettleSeconds = 6
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Out | Out-Null
$out = (Resolve-Path $Out).Path
$copy = Join-Path $out "pulse-shot-copy.exe"
Copy-Item (Resolve-Path $Exe).Path $copy -Force

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public static List<string[]> Windows(uint pid) {
    var list = new List<string[]>();
    EnumWindows((h, p) => {
      uint q; GetWindowThreadProcessId(h, out q);
      if (q == pid) { var s = new StringBuilder(256); GetWindowText(h, s, 256); list.Add(new string[] { h.ToInt64().ToString(), s.ToString(), IsWindowVisible(h) ? "1" : "0" }); }
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
"@
[void][W]::SetProcessDPIAware()

function Blank($bmp) {
  $seen = @{}
  for ($y = 6; $y -lt $bmp.Height; $y += 17) { for ($x = 6; $x -lt $bmp.Width; $x += 13) { $seen[$bmp.GetPixel($x, $y).ToArgb()] = 1 } }
  return $seen.Count -lt 6
}
function Capture($h) {
  $r = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object Drawing.Bitmap $w, $hh
  $g = [Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
  [void][W]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc); $g.Dispose()
  if (Blank $bmp) {
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object Drawing.Size $w, $hh))
    $g.Dispose()
  }
  return $bmp
}

$caseList = Get-Content -Raw $CasesFile | ConvertFrom-Json
$fg0 = [W]::GetForegroundWindow()
$area = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$report = [ordered]@{ workArea = "$($area.X),$($area.Y) $($area.Width)x$($area.Height)"; shots = [ordered]@{} }
$run = 0
foreach ($case in $caseList) {
  $run++
  $name = $case.name
  $keys = @()
  foreach ($prop in $case.env.PSObject.Properties) { Set-Item "env:$($prop.Name)" ([string]$prop.Value); $keys += $prop.Name }
  $env:PULSE_SHOW = "1"
  $env:PULSE_SETTINGS_DIR = Join-Path $out "settings\run$run"
  New-Item -ItemType Directory -Force $env:PULSE_SETTINGS_DIR | Out-Null
  # Optional "settings": an object written as settings.json before the start.
  if ($case.settings) { $case.settings | ConvertTo-Json | Set-Content (Join-Path $env:PULSE_SETTINGS_DIR "settings.json") }
  $env:PULSE_INSTANCE_NAME = "Pulse-shots-" + [guid]::NewGuid().ToString("N")
  $proc = $null
  try {
    $proc = Start-Process $copy -PassThru
  } finally {
    foreach ($k in $keys + @("PULSE_SHOW", "PULSE_SETTINGS_DIR", "PULSE_INSTANCE_NAME")) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
  }
  try {
    $h = [IntPtr]::Zero
    $end = (Get-Date).AddSeconds(45)
    while ((Get-Date) -lt $end -and $h -eq [IntPtr]::Zero) {
      $pick = [W]::Windows([uint32]$proc.Id) | Where-Object { $_[2] -eq "1" -and $_[1] -ne "" } | Select-Object -First 1
      if ($pick) { $h = [IntPtr][int64]$pick[0] } else { Start-Sleep -Milliseconds 200 }
    }
    if ($h -eq [IntPtr]::Zero) { Write-Output "no window for $name"; continue }
    Start-Sleep -Seconds $SettleSeconds
    $bmp = Capture $h
    $png = Join-Path $out "$name.png"
    $bmp.Save($png, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    $r = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$r)
    $ex = [int64][W]::GetWindowLongPtrW($h, -20)
    # The work area of the monitor the window is on (the cursor's monitor).
    $wa = [System.Windows.Forms.Screen]::FromHandle($h).WorkingArea
    $report.shots[$name] = [ordered]@{
      png = $png
      window = "$($r.L),$($r.T) to $($r.R),$($r.B)"
      sizePx = "$($r.R - $r.L)x$($r.B - $r.T)"
      dpi = [W]::GetDpiForWindow($h)
      workArea = "$($wa.X),$($wa.Y) $($wa.Width)x$($wa.Height)"
      gapRight = ($wa.X + $wa.Width) - $r.R
      gapBottom = ($wa.Y + $wa.Height) - $r.B
      toolWindow = (($ex -band 0x80) -ne 0)
    }
    Write-Output "$name $($report.shots[$name].sizePx) gap $($report.shots[$name].gapRight),$($report.shots[$name].gapBottom)"
  } finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    if ($proc) { try { Wait-Process -Id $proc.Id -Timeout 10 -ErrorAction SilentlyContinue } catch {} }
  }
}
$report.foregroundUnchanged = ([W]::GetForegroundWindow() -eq $fg0)
$report | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $out "shots.json")
