# Starts a test copy of Pulse and polls its top-level windows by process id as
# fast as it can for the first seconds, logging every change of visibility,
# extended style and owner. A window that is ever visible with
# WS_EX_APPWINDOW, or visible with neither WS_EX_TOOLWINDOW nor an owner, is
# eligible for a taskbar button and an Alt+Tab entry: that is the bug this
# looks for. (An owned window without WS_EX_APPWINDOW is not eligible even
# without WS_EX_TOOLWINDOW; the lines still show it.)
# Sends no input, never takes the focus, stops only the process it started.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\watch-first-window.ps1 `
#     -Exe <pulse-native.exe> -Out <folder> [-Seconds 2] [-Runs 5]
param(
  [Parameter(Mandatory)] [string]$Exe,
  [Parameter(Mandatory)] [string]$Out,
  [double]$Seconds = 2.0,
  [int]$Runs = 5
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $Out | Out-Null
$out = (Resolve-Path $Out).Path
$copy = Join-Path $out "pulse-watch-copy.exe"
Copy-Item (Resolve-Path $Exe).Path $copy -Force

Add-Type @"
using System; using System.Collections.Generic; using System.Diagnostics; using System.Runtime.InteropServices; using System.Text;
public static class Watch {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetLong(IntPtr h, int i);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
  public class Seen { public bool Visible; public long Ex; }
  // Returns the log lines. Starts the process itself so no time is lost.
  public static List<string> Run(string exe, Dictionary<string,string> env, double seconds, out int pid, out int polls, out int badStates) {
    var psi = new ProcessStartInfo(exe) { UseShellExecute = false };
    foreach (var kv in env) psi.EnvironmentVariables[kv.Key] = kv.Value;
    var sw = Stopwatch.StartNew();
    var p = Process.Start(psi);
    pid = p.Id;
    var log = new List<string>();
    var seen = new Dictionary<long, Seen>();
    int n = 0, bad = 0;
    uint want = (uint)p.Id;
    while (sw.Elapsed.TotalSeconds < seconds) {
      n++;
      double t = sw.Elapsed.TotalMilliseconds;
      var found = new List<IntPtr>();
      EnumWindows((h, x) => { uint q; GetWindowThreadProcessId(h, out q); if (q == want) found.Add(h); return true; }, IntPtr.Zero);
      foreach (var h in found) {
        bool vis = IsWindowVisible(h);
        long ex = GetLong(h, -20).ToInt64();
        Seen s;
        if (!seen.TryGetValue(h.ToInt64(), out s) || s.Visible != vis || s.Ex != ex) {
          seen[h.ToInt64()] = new Seen { Visible = vis, Ex = ex };
          RECT r; GetWindowRect(h, out r);
          var cls = new StringBuilder(128); GetClassName(h, cls, 128);
          bool tool = (ex & 0x80) != 0, app = (ex & 0x40000) != 0;
          bool owned = GetWindow(h, 4) != IntPtr.Zero;
          bool wrong = vis && (app || (!tool && !owned));
          if (wrong) bad++;
          log.Add(string.Format("{0,8:F2} ms  hwnd {1,8:X}  {2,-28} visible={3,-5} exstyle=0x{4:X8} toolwindow={5,-5} appwindow={6,-5} rect={7},{8} {9}x{10} owner={11}{12}",
            t, h.ToInt64(), cls, vis, ex, tool, app, r.L, r.T, r.R - r.L, r.B - r.T, GetWindow(h, 4) != IntPtr.Zero, wrong ? "   <-- ELIGIBLE FOR THE TASKBAR / ALT+TAB" : (vis && !tool ? "   (visible, owned, no APPWINDOW: not eligible)" : "")));
        }
      }
    }
    polls = n; badStates = bad;
    return log;
  }
}
"@

$summary = @()
for ($i = 1; $i -le $Runs; $i++) {
  $settings = Join-Path $out "watch-settings\run$i"
  New-Item -ItemType Directory -Force $settings | Out-Null
  $env_ = New-Object "System.Collections.Generic.Dictionary[string,string]"
  foreach ($e in Get-ChildItem env:) { $env_[$e.Name] = $e.Value }
  $env_["PULSE_SAMPLE"] = "1"
  $env_["PULSE_SETTINGS_DIR"] = $settings
  $env_["PULSE_INSTANCE_NAME"] = "Pulse-watch-" + [guid]::NewGuid().ToString("N")
  $procId = 0; $polls = 0; $bad = 0
  $log = [Watch]::Run($copy, $env_, $Seconds, [ref]$procId, [ref]$polls, [ref]$bad)
  $line = "run ${i}: pid $procId, $polls polls in $Seconds s, windows changes: $($log.Count), eligible for taskbar/Alt+Tab while visible: $bad"
  Write-Output $line
  $log | ForEach-Object { Write-Output "    $_" }
  $summary += $line
  $summary += ($log | ForEach-Object { "    $_" })
  # Stop only the process started above, by its recorded id.
  Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
  try { Wait-Process -Id $procId -Timeout 10 -ErrorAction SilentlyContinue } catch {}
}
$summary | Set-Content (Join-Path $out "watch-first-window.log")
