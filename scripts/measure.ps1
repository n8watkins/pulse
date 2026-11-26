# Measures a Pulse shell (the webview one, the native one, any exe that follows
# the environment hooks below), without sending keys, clicking, touching the
# clipboard or taking the focus. Every wait is bounded, there is an overall
# limit, and only the processes this script started are stopped.
#
# Hooks the exe must follow:
#   PULSE_SAMPLE=1                use the sample readings
#   PULSE_SHOW=1                  show the panel at start without activating it
#   PULSE_THEME=light|dark        force the theme
#   PULSE_NOTRIM=1                do not trim memory on hide (the default trims)
#   PULSE_SELFTEST=1 PULSE_REPORT=<path>
#                                 open and close the panel 5 times, write
#                                 {"shell":..., "open_ms":[...]} and keep running
# -View expanded|compact (and -WslOpen 0|1) start every run in that view, for
# comparing the views' memory with PULSE_VIEW and PULSE_WSL_OPEN.
# Every start also gets its own PULSE_SETTINGS_DIR (a scratch folder under -Out)
# and PULSE_INSTANCE_NAME (a unique name), so measuring never touches the real
# settings, the running Pulse or the Run key.
# The panel window is titled "Pulse" (a visible top-level window of the process
# is used when no window has that title).
#
# Writes to -Out: report.json, panel-light.png and panel-dark.png (sample
# readings) and panel-light-real.png and panel-dark-real.png (the real reader).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\measure.ps1 `
#     -Exe target\release\pulse-webview.exe -Out C:\path\to\out
param(
  [Parameter(Mandatory)] [string]$Exe,
  [Parameter(Mandatory)] [string]$Out,
  [int]$IdleSeconds = 90,
  [int]$SettleSeconds = 20,
  [int]$LimitSeconds = 1800,
  # "expanded" or "compact": start every run in that view (PULSE_VIEW); empty leaves the default.
  [string]$View = "",
  # "0" or "1": the compact view's Linux list closed or open (PULSE_WSL_OPEN).
  [string]$WslOpen = ""
)
$ErrorActionPreference = "Stop"
$exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $Out | Out-Null
$out = (Resolve-Path $Out).Path
$started = Get-Date
$report = [ordered]@{ exe = $exe; startedAt = $started.ToString("s"); errors = @() }
$pidFile = Join-Path $out "started-pids.txt"
Set-Content $pidFile ""

Add-Type -AssemblyName System.Drawing
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
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int n);
  // The process's top-level windows: handle, title, visible.
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

function Log($text) { $line = "{0:HH:mm:ss} {1}" -f (Get-Date), $text; Write-Output $line; Add-Content (Join-Path $out "measure.log") $line }
function Check-Time { if (((Get-Date) - $started).TotalSeconds -gt $LimitSeconds) { throw "over the time limit of $LimitSeconds s" } }
function Wait-Until($what, [scriptblock]$test, [int]$seconds) {
  $end = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $end) { Check-Time; $v = & $test; if ($v) { return $v }; Start-Sleep -Milliseconds 200 }
  throw "timed out waiting for $what"
}
function Pause-Bounded([double]$seconds) { Check-Time; Start-Sleep -Milliseconds ([int]($seconds * 1000)) }

# ----- processes: start, tree, stop -----
function Descendants([int]$root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
  $tree = New-Object System.Collections.Generic.List[int]; $tree.Add($root)
  $added = $true
  while ($added) { $added = $false; foreach ($p in $all) { if ($tree.Contains([int]$p.ParentProcessId) -and -not $tree.Contains([int]$p.ProcessId)) { $tree.Add([int]$p.ProcessId); $added = $true } } }
  return , $tree.ToArray()
}

$all_started = New-Object System.Collections.Generic.List[int]
$runs = 0
function Start-Shell([hashtable]$envVars) {
  foreach ($k in @("PULSE_SAMPLE", "PULSE_SHOW", "PULSE_THEME", "PULSE_SELFTEST", "PULSE_REPORT", "PULSE_NOTRIM", "PULSE_SETTINGS_DIR", "PULSE_INSTANCE_NAME", "PULSE_VIEW", "PULSE_WSL_OPEN")) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
  foreach ($k in $envVars.Keys) { Set-Item "env:$k" $envVars[$k] }
  if ($View) { $env:PULSE_VIEW = $View }
  if ($WslOpen) { $env:PULSE_WSL_OPEN = $WslOpen }
  # A scratch settings folder and a unique instance name for every start.
  $runNo = $script:runs + 1
  $env:PULSE_SETTINGS_DIR = Join-Path $out "settings\run$runNo"
  New-Item -ItemType Directory -Force $env:PULSE_SETTINGS_DIR | Out-Null
  $env:PULSE_INSTANCE_NAME = "Pulse-measure-" + [guid]::NewGuid().ToString("N")
  # A fresh WebView2 profile folder per start (ignored by shells without a
  # webview), so a run never waits on the previous run's dying processes.
  $script:runs++
  $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $out "profiles\run$($script:runs)"
  try { $p = Start-Process $exe -PassThru } finally {
    foreach ($k in $envVars.Keys) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
    Remove-Item env:PULSE_SETTINGS_DIR, env:PULSE_INSTANCE_NAME, env:PULSE_VIEW, env:PULSE_WSL_OPEN -ErrorAction SilentlyContinue
    Remove-Item env:WEBVIEW2_USER_DATA_FOLDER -ErrorAction SilentlyContinue
  }
  $all_started.Add($p.Id); Add-Content $pidFile $p.Id
  return $p
}
function Stop-Shell($p) {
  if (-not $p) { return }
  $tree = Descendants $p.Id
  # Children first, the root last; only this run's tree.
  [array]::Reverse($tree)
  foreach ($id in $tree) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
  try { Wait-Process -Id $p.Id -Timeout 10 -ErrorAction SilentlyContinue } catch {}
  # Wait until the whole tree is gone before the next start.
  $end = (Get-Date).AddSeconds(15)
  while ((Get-Date) -lt $end -and ($tree | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })) { Start-Sleep -Milliseconds 200 }
}

# ----- memory: Task Manager's number, "Working Set - Private" -----
function Memory([int]$root) {
  $tree = Descendants $root
  # A process that exits during the sweep makes the whole read fail: read again.
  $samples = $null
  for ($try = 0; $try -lt 10 -and -not $samples; $try++) {
    try { $samples = (Get-Counter -Counter "\Process V2(*)\Working Set - Private" -ErrorAction Stop).CounterSamples } catch { $lastErr = $_.Exception.Message; Start-Sleep -Milliseconds 1000 }
  }
  if (-not $samples) { throw "the Working Set - Private counter could not be read: $lastErr" }
  $private = 0.0; $found = 0; $rows = @(); $host_ = 0.0
  foreach ($s in $samples) {
    $i = $s.InstanceName.LastIndexOf(":")
    if ($i -lt 0) { continue }
    $id = 0; if (-not [int]::TryParse($s.InstanceName.Substring($i + 1), [ref]$id)) { continue }
    if (($tree -contains $id) -and $s.Status -eq 0) {
      $private += $s.CookedValue; $found++
      if ($id -eq $root) { $host_ = $s.CookedValue }
      $rows += [ordered]@{ pid = $id; name = $s.InstanceName.Substring(0, $i); privateWorkingSetMb = [math]::Round($s.CookedValue / 1MB, 1) }
    }
  }
  $commit = 0.0
  foreach ($id in $tree) { $pr = Get-Process -Id $id -ErrorAction SilentlyContinue; if ($pr) { $commit += $pr.PrivateMemorySize64 } }
  return [ordered]@{ privateWorkingSetMb = [math]::Round($private / 1MB, 1); commitMb = [math]::Round($commit / 1MB, 1); processes = $tree.Count; counted = $found; hostPrivateWorkingSetMb = [math]::Round($host_ / 1MB, 1); childrenPrivateWorkingSetMb = [math]::Round(($private - $host_) / 1MB, 1); perProcess = $rows }
}
function Memory-Series([int]$root, [int]$count = 5, [int]$gap = 5) {
  $list = @()
  for ($i = 0; $i -lt $count; $i++) { if ($i -gt 0) { Pause-Bounded $gap }; 
    try { $list += , (Memory $root) } catch { $report.errors += "memory sample skipped: $($_.Exception.Message)" }
  }
  if (-not $list.Count) { throw "no memory sample could be taken" }
  $avg = { param($k) [math]::Round((($list | ForEach-Object { $_[$k] } | Measure-Object -Average).Average), 1) }
  return [ordered]@{ samples = $list; meanPrivateWorkingSetMb = (& $avg "privateWorkingSetMb"); meanCommitMb = (& $avg "commitMb") }
}

# ----- the window -----
function Find-Window([int]$procId, [bool]$visibleOnly) {
  $list = [W]::Windows([uint32]$procId)
  $pick = $list | Where-Object { $_[1] -eq "Pulse" -and ((-not $visibleOnly) -or $_[2] -eq "1") } | Select-Object -First 1
  if (-not $pick -and $visibleOnly) {
    $pick = $list | Where-Object { $_[2] -eq "1" -and $_[1] -ne "" } | Select-Object -First 1
  }
  if ($pick) { return [IntPtr][int64]$pick[0] }
  return [IntPtr]::Zero
}
function Bounds($h) { $r = New-Object W+RECT; [void][W]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); if ($r.R -le $r.L) { [void][W]::GetWindowRect($h, [ref]$r) }; return $r }
function Blank($bmp) {
  $seen = @{}
  for ($y = 6; $y -lt $bmp.Height; $y += 17) { for ($x = 6; $x -lt $bmp.Width; $x += 13) { $seen[$bmp.GetPixel($x, $y).ToArgb()] = 1 } }
  return $seen.Count -lt 6
}
function Capture($h) {
  $r = Bounds $h; $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object Drawing.Bitmap $w, $hh
  $g = [Drawing.Graphics]::FromImage($bmp); $dc = $g.GetHdc()
  [void][W]::PrintWindow($h, $dc, 2); $g.ReleaseHdc($dc); $g.Dispose()
  if (Blank $bmp) {
    # GPU-composited content is often missed by PrintWindow: copy the window's
    # own rectangle from the screen (it is topmost and shown without activation).
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object Drawing.Size $w, $hh))
    $g.Dispose()
  }
  return $bmp
}

# Stops anything this run left behind if the script itself is killed.
$watch = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList "-NoProfile", "-Command", "Start-Sleep $($LimitSeconds + 60); Get-Content '$pidFile' | Where-Object { `$_ } | ForEach-Object { Stop-Process -Id ([int]`$_) -Force -ErrorAction SilentlyContinue }"
$proc = $null
try {
  Log "START (limit $LimitSeconds s, idle wait $IdleSeconds s)"
  Add-Type -AssemblyName System.Windows.Forms
  $fg0 = [W]::GetForegroundWindow()
  $area = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
  $report.workArea = "$($area.X),$($area.Y) $($area.Width)x$($area.Height)"

  # (a) Hidden: the panel never shown, sample readings.
  # Trimmed (the default) and with PULSE_NOTRIM=1 (a shell that trims itself
  # on hide stops doing so).
  foreach ($trim in @("trim", "notrim")) {
    Log "hidden ($trim): start, wait $IdleSeconds s"
    $vars = @{ PULSE_SAMPLE = "1" }
    if ($trim -eq "notrim") { $vars.PULSE_NOTRIM = "1" }
    $proc = Start-Shell $vars
    Pause-Bounded $IdleSeconds
    if ($proc.HasExited) { throw "the exe exited while hidden (code $($proc.ExitCode))" }
    $key = if ($trim -eq "trim") { "hidden" } else { "hiddenNoTrim" }
    $report[$key] = Memory-Series $proc.Id
    $report[$key].visibleWindows = @([W]::Windows([uint32]$proc.Id) | Where-Object { $_[2] -eq "1" }).Count
    Log ("hidden ({0}): private working set {1} MB, commit {2} MB" -f $trim, $report[$key].meanPrivateWorkingSetMb, $report[$key].meanCommitMb)
    Stop-Shell $proc; $proc = $null
  }

  # (b) Self-test: sample readings, the real reader, and sample without the trim.
  foreach ($mode in @("sample", "real", "sample-notrim")) {
    $file = Join-Path $out "selftest-$mode.json"
    Remove-Item $file -ErrorAction SilentlyContinue
    $vars = @{ PULSE_SELFTEST = "1"; PULSE_REPORT = $file }
    if ($mode -ne "real") { $vars.PULSE_SAMPLE = "1" }
    if ($mode -eq "sample-notrim") { $vars.PULSE_NOTRIM = "1" }
    Log "selftest ($mode)"
    $proc = Start-Shell $vars
    try { Wait-Until "the self-test report" { Test-Path $file } 90 | Out-Null } catch {
      Log ("windows of pid {0}: {1}" -f $proc.Id, (([W]::Windows([uint32]$proc.Id) | ForEach-Object { "[" + ($_ -join "|") + "]" }) -join " "))
      Log ("alive: {0}, tree: {1}" -f (-not $proc.HasExited), ((Descendants $proc.Id) -join ","))
      throw
    }
    Start-Sleep -Milliseconds 300
    $j = Get-Content -Raw $file | ConvertFrom-Json
    $ok = @($j.open_ms | Where-Object { $_ -ne $null })
    $sorted = @($ok | Sort-Object)
    $report["selftest_$mode"] = [ordered]@{ shell = $j.shell; open_ms = $j.open_ms; medianMs = $(if ($sorted.Count) { $sorted[[int][math]::Floor(($sorted.Count - 1) / 2)] } else { $null }); maxMs = $(if ($sorted.Count) { $sorted[-1] } else { $null }); stillRunning = (-not $proc.HasExited) }
    Log ("selftest {0}: {1}" -f $mode, ($j.open_ms -join ", "))
    Stop-Shell $proc; $proc = $null
  }

  # (c) Open: shown without activation, each theme.
  $report.open = [ordered]@{}
  # Sample readings first, then the real reader (real icons and names).
  foreach ($case in @(@("light", "sample"), @("dark", "sample"), @("light", "real"), @("dark", "real"))) {
    $theme = $case[0]; $mode = $case[1]
    $label = if ($mode -eq "sample") { $theme } else { "$theme-real" }
    Log "open: $label"
    $vars = @{ PULSE_SHOW = "1"; PULSE_THEME = $theme }
    if ($mode -eq "sample") { $vars.PULSE_SAMPLE = "1" }
    $proc = Start-Shell $vars
    try {
      $h = Wait-Until "the visible window" { $w = Find-Window $proc.Id $true; if ($w -ne [IntPtr]::Zero) { $w } } 45
    } catch {
      Log ("windows of pid {0}: {1}" -f $proc.Id, (([W]::Windows([uint32]$proc.Id) | ForEach-Object { "[" + ($_ -join "|") + "]" }) -join " "))
      throw
    }
    $bmp = $null
    Wait-Until "the first frame" { $b = Capture $h; if (-not (Blank $b)) { $script:bmp = $b; $true } else { $b.Dispose(); $false } } 30 | Out-Null
    # Let the sparklines fill and the icons arrive, then take the picture and the memory.
    Pause-Bounded $SettleSeconds
    $bmp.Dispose()
    $bmp = Capture $h
    $png = Join-Path $out "panel-$label.png"
    $bmp.Save($png, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    $r = Bounds $h
    $entry = [ordered]@{ png = $png }
    $entry.window = "$($r.L),$($r.T) to $($r.R),$($r.B)"
    $entry.sizePx = "$($r.R - $r.L)x$($r.B - $r.T)"
    $entry.dpi = [W]::GetDpiForWindow($h)
    $entry.gapRight = ($area.X + $area.Width) - $r.R
    $entry.gapBottom = ($area.Y + $area.Height) - $r.B
    # (d) Extended style: a tool window, never an app window.
    $ex = [int64][W]::GetWindowLongPtrW($h, -20)
    $entry.toolWindow = (($ex -band 0x80) -ne 0)
    $entry.appWindow = (($ex -band 0x40000) -ne 0)
    $entry.styleOk = ($entry.toolWindow -and -not $entry.appWindow)
    $entry.memory = Memory-Series $proc.Id
    Log ("open {0}: private working set {1} MB, commit {2} MB, tool window {3}" -f $label, $entry.memory.meanPrivateWorkingSetMb, $entry.memory.meanCommitMb, $entry.styleOk)
    $report.open[$label] = $entry
    Stop-Shell $proc; $proc = $null
  }
  $report.foregroundUnchanged = ([W]::GetForegroundWindow() -eq $fg0)
  Log "END"
} catch {
  Log "STOPPED: $($_.Exception.Message)"
  $report.errors += $_.Exception.Message
} finally {
  if ($proc) { Stop-Shell $proc }
  foreach ($id in $all_started) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
  if ($watch) { Stop-Process -Id $watch.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 500
  # The profile folders this run made.
  Remove-Item (Join-Path $out "profiles") -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item (Join-Path $out "settings") -Recurse -Force -ErrorAction SilentlyContinue
  $report.seconds = [int]((Get-Date) - $started).TotalSeconds
  $report | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $out "report.json")
}
