# Read or set the position and size of a process's top-level windows (physical
# pixels), for the window checks that need a real move and resize.
#   powershell -File winrect.ps1 <pid>                  list visible windows: "x y w h title"
#   powershell -File winrect.ps1 <pid> x y w h          move/resize the largest one, then list
param([int]$ProcessId, [int]$X = [int]::MinValue, [int]$Y = 0, [int]$W = 0, [int]$H = 0)
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class Win {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<IntPtr> Of(uint pid) {
    var found = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) found.Add(h); return true; }, IntPtr.Zero);
    return found;
  }
}
"@
[void][Win]::SetProcessDPIAware()
$wins = [Win]::Of([uint32]$ProcessId)
function Info($h) {
  $r = New-Object Win+RECT
  [void][Win]::GetWindowRect($h, [ref]$r)
  $t = New-Object System.Text.StringBuilder 256
  [void][Win]::GetWindowText($h, $t, 256)
  [pscustomobject]@{ H = $h; X = $r.L; Y = $r.T; W = $r.R - $r.L; Hh = $r.B - $r.T; Title = $t.ToString() }
}
$all = @($wins | ForEach-Object { Info $_ } | Where-Object { $_.W -gt 100 -and $_.Hh -gt 100 })
if ($X -ne [int]::MinValue -and $all.Count -gt 0) {
  $big = $all | Sort-Object { $_.W * $_.Hh } -Descending | Select-Object -First 1
  [void][Win]::SetWindowPos($big.H, [IntPtr]::Zero, $X, $Y, $W, $H, 0x0014)  # no z-order change, no activate
  Start-Sleep -Milliseconds 200
  $all = @($wins | ForEach-Object { Info $_ } | Where-Object { $_.W -gt 100 -and $_.Hh -gt 100 })
}
$all | ForEach-Object { "{0} {1} {2} {3} {4}" -f $_.X, $_.Y, $_.W, $_.Hh, $_.Title }
