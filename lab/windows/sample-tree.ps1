# Samples every 5 s: the MercuryChat desktop app's process tree (mercurychat.exe
# plus its WebView2 descendants) and each mercury.exe. One line per sample:
#   HH:mm:ss app procs=N private_mb=.. ws_mb=.. cpu_s=.. | mercury <pid> ws_mb=.. cpu_s=.. ...
# Stops when -StopFile exists.
param([string]$Out, [string]$StopFile, [int]$Seconds = 5)
$ErrorActionPreference = 'SilentlyContinue'
while (-not (Test-Path $StopFile)) {
    $all = Get-CimInstance Win32_Process
    $byParent = @{}
    foreach ($p in $all) {
        $k = [int]$p.ParentProcessId
        if (-not $byParent.ContainsKey($k)) { $byParent[$k] = @() }
        $byParent[$k] += $p
    }
    $roots = $all | Where-Object { $_.Name -eq 'mercurychat.exe' }
    $tree = @()
    $queue = New-Object System.Collections.Queue
    foreach ($r in $roots) { $queue.Enqueue($r) }
    while ($queue.Count -gt 0) {
        $p = $queue.Dequeue(); $tree += $p
        foreach ($c in $byParent[[int]$p.ProcessId]) { $queue.Enqueue($c) }
    }
    $priv = ($tree | Measure-Object -Property PrivatePageCount -Sum).Sum / 1MB
    $ws = ($tree | Measure-Object -Property WorkingSetSize -Sum).Sum / 1MB
    $cpu = (($tree | ForEach-Object { [double]$_.KernelModeTime + [double]$_.UserModeTime }) | Measure-Object -Sum).Sum / 1e7
    $line = '{0} app roots={1} procs={2} private_mb={3:N1} ws_mb={4:N1} cpu_s={5:N2} |' -f `
        (Get-Date).ToUniversalTime().ToString('HH:mm:ss'), @($roots).Count, $tree.Count, $priv, $ws, $cpu
    foreach ($m in ($all | Where-Object { $_.Name -eq 'mercury.exe' })) {
        $line += ' mercury {0} ws_mb={1:N1} cpu_s={2:N2}' -f $m.ProcessId, ($m.WorkingSetSize / 1MB), (([double]$m.KernelModeTime + [double]$m.UserModeTime) / 1e7)
    }
    Add-Content -Path $Out -Value $line -Encoding utf8
    Start-Sleep -Seconds $Seconds
}
