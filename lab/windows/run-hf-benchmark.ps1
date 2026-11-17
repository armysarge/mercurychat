param(
    [Parameter(Mandatory)][ValidateSet('Mercury','VARA')][string]$Modem,
    [Parameter(Mandatory)][string]$Output,
    [int]$Bytes = 8192,
    [int]$Timeout = 90,
    [int]$ConnectTimeout = $Timeout,
    [ValidateSet('Direct','WGN','IMPULSE','MPG','MPM','MPP','MPD')][string]$Channel = 'Direct',
    [double[]]$Noise,
    [double[]]$CalibratedPeaks,
    [int]$Seed = 4242,
    [ValidateRange(-30,0)][double]$RxGainDb = 0,
    [int[]]$VaraRxChannels = @(0,1),
    [switch]$ProcessLoopback,
    [switch]$TraceMercury,
    [switch]$Registered,
    [string]$MercuryExe = "$PSScriptRoot/../bin-patched/windows-x64/mercury.exe",
    [string]$VaraDir = 'C:\VARA',
    [string]$Engine = "$PSScriptRoot/../run/hf-benchmark-sources/ionos_pipe_48k_gain.exe",
    [string]$CaptureExe = "$PSScriptRoot/../run/hf-benchmark-sources/hf-process-audio.exe",
    [string]$AudioPython = 'C:\VARA\OpenHF\.venv\Scripts\python.exe',
    [string]$Python = 'python',
    [string]$MercuryTxDevice = '{0.0.0.00000000}.{3ff006d9-7e73-4e61-a99c-43eb6018e308}',
    [string[]]$MercuryRxDevices = @('{0.0.1.00000000}.{22742773-d4c6-4f25-bdd3-686ec5e365cb}','{0.0.1.00000000}.{a3544a95-51f9-4cdc-aee9-359bd5b81053}'),
    [int]$PayloadSeed = 4242,
    [double]$IntegrityGrace = 0,
    [switch]$Aligned,
    [switch]$Normalize,
    [switch]$Reverse,
    [switch]$DiscardVaraRuntime
)

$ErrorActionPreference = 'Stop'
$invariant = [Globalization.CultureInfo]::InvariantCulture
$varaFiles = @('VARA.exe','Data.dat','VARAHF500.dat','VARAHF2300.dat','VARAHF2750.dat')
if ($Modem -eq 'Mercury') { $MercuryExe = (Resolve-Path -LiteralPath $MercuryExe).Path }
if ($MercuryRxDevices.Count -ne 2) { throw 'Specify both Mercury receive devices in A, B order' }
if ($Channel -ne 'Direct') {
    $Engine = (Resolve-Path -LiteralPath $Engine).Path
    if ($ProcessLoopback) { $CaptureExe = (Resolve-Path -LiteralPath $CaptureExe).Path }
}
$out = [IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $out) { throw "Use a new result directory: $out" }
$null = New-Item -ItemType Directory -Path $out
$ports = @(19300,19400)
$benchPorts = @(19300,19301,19400,19401)
if (Get-NetTCPConnection -State Listen -LocalPort $benchPorts -ErrorAction SilentlyContinue) {
    throw 'A benchmark TCP port is already in use'
}
$processes = @()
$bridge = $null
$calls = @('ZS1AAA','ZS2BBB')
if ($Registered) {
    $licence = Select-String -LiteralPath (Join-Path $VaraDir 'VARA.ini') -Pattern '^Callsign Licence 0=([A-Z0-9]{3,7})$'
    if (!$licence) { throw 'No usable registered VARA callsign was found' }
    $baseCall = $licence.Matches[0].Groups[1].Value
    $calls = @("$baseCall-1","$baseCall-2")
}
try {
    foreach ($index in 0,1) {
        $dir = Join-Path $out "station-$index"
        $null = New-Item -ItemType Directory -Path $dir
        $port = $ports[$index]
        if ($Modem -eq 'Mercury') {
            $exe = $MercuryExe
            $outputs = @('{0.0.0.00000000}.{1f68aa44-9ff8-4523-8429-02de487b53e0}','{0.0.0.00000000}.{964afa2f-cfaa-4fca-90ad-66241370063e}')
            $inputs = $MercuryRxDevices
            $capture = 'left'
            if ($ProcessLoopback) {
                $outputs = @($MercuryTxDevice) * 2
            } elseif ($Channel -ne 'Direct') {
                $inputs = @('{0.0.1.00000000}.{d627c42d-0956-4bcb-a6a6-f38467136592}') * 2
                $capture = @('left','right')[$index]
            }
            @'
[main]
bind_address = 127.0.0.1
ui_enabled = false
[ptt]
method = none
[store]
enabled = false
'@ | Set-Content -LiteralPath (Join-Path $dir 'mercury.ini') -Encoding ascii
            $mercuryArgs = @('-C','mercury.ini','-x','wasapi','-i',$inputs[$index],'-o',$outputs[$index],'-k',$capture,
                             '-P','none','-p',$port,'-b',($port + 10),'-L','mercury.log')
            if ($TraceMercury) { $mercuryArgs += '-v' }
            $processes += Start-Process -FilePath $exe -ArgumentList $mercuryArgs -WorkingDirectory $dir -WindowStyle Hidden -PassThru `
                -RedirectStandardOutput (Join-Path $dir 'stdout.log') -RedirectStandardError (Join-Path $dir 'stderr.log')
        } else {
            foreach ($name in $varaFiles) {
                Copy-Item -LiteralPath (Join-Path $VaraDir $name) -Destination $dir
            }
            $ini = [IO.File]::ReadAllText((Join-Path $VaraDir 'VARA.ini'), [Text.Encoding]::Latin1)
            $input = @('CABLE-B Output (VB-Audio Cable ','CABLE-A Output (VB-Audio Cable ')[$index]
            $output = @('CABLE-A Input (VB-Audio Cable A','CABLE-B Input (VB-Audio Cable B')[$index]
            $virtualPort = @('COM2','COM7')[$index]
            $capture = 0
            if ($ProcessLoopback) {
                $output = 'CABLE Input (VB-Audio Virtual C'
            } elseif ($Channel -ne 'Direct') {
                $input = 'CABLE Output (VB-Audio Virtual'
                $capture = $VaraRxChannels[$index]
            }
            foreach ($entry in @{
                'Input Device Name' = $input
                'Output Device Name' = $output
                'Channel' = $capture
                'RA-Board Device Path' = ''
                'Rig' = '0'
                'PTTPort' = $virtualPort
                'CATPort' = $virtualPort
                'RTS' = '0'
                'DTR' = '0'
                'Via' = '0'
                'TCP Command Port' = $port
                'RA-Board PTT' = '0'
                'Updates' = '0'
                'CW ID' = '0'
                'ATU' = '0'
                'Enable KISS' = '0'
                'Enable SysLog' = '0'
                'CommandsLog' = '1'
            }.GetEnumerator()) {
                $pattern = '(?m)^' + [regex]::Escape($entry.Key) + '=[^\r\n]*'
                $ini = [regex]::Replace($ini, $pattern, ($entry.Key + '=' + $entry.Value))
            }
            [IO.File]::WriteAllText((Join-Path $dir 'VARA.ini'), $ini, [Text.Encoding]::Latin1)
            $exe = Join-Path $dir 'VARA.exe'
            $processes += Start-Process -FilePath $exe -WorkingDirectory $dir -WindowStyle Hidden -PassThru
        }
        ('sha256=' + (Get-FileHash -LiteralPath $exe).Hash) | Set-Content -LiteralPath (Join-Path $dir 'binary.txt')
    }
    if ($Channel -ne 'Direct') {
        $audio = Join-Path $out 'audio'
        $null = New-Item -ItemType Directory -Path $audio
        $bridgeArgs = @((Join-Path $PSScriptRoot 'hf-ionos-bridge.py'), '--engine',
            $Engine, '--output', $audio, '--mode', $Channel,
            '--seed', $Seed, '--rx-gain-db', $RxGainDb.ToString($invariant))
        if ($ProcessLoopback) {
            $bridgeArgs += @('--loopback',$CaptureExe,
                             '--pids',$processes[0].Id,$processes[1].Id)
        }
        if ($Noise) {
            if ($Noise.Count -ne 3) { throw 'Specify SNR, station A peak and station B peak' }
            $numeric = @($Noise | ForEach-Object { $_.ToString($invariant) })
            $bridgeArgs += @('--snr',$numeric[0],'--peaks',$numeric[1],$numeric[2])
        } elseif ($CalibratedPeaks) {
            if ($CalibratedPeaks.Count -ne 2) { throw 'Specify station A and station B calibrated peaks' }
            $numeric = @($CalibratedPeaks | ForEach-Object { $_.ToString($invariant) })
            $bridgeArgs += @('--peaks',$numeric[0],$numeric[1])
        }
        if ($Aligned) { $bridgeArgs += '--align' }
        if ($Normalize) { $bridgeArgs += '--normalize' }
        $bridgeArgs = $bridgeArgs | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }
        $bridge = Start-Process -FilePath $AudioPython -ArgumentList $bridgeArgs `
            -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $audio 'stderr.log')
        for ($i=0; $i -lt 15 -and !(Test-Path -LiteralPath (Join-Path $audio 'ready')); $i++) {
            if ($bridge.HasExited) { break }
            Start-Sleep -Seconds 1
        }
        if (!(Test-Path -LiteralPath (Join-Path $audio 'ready'))) { throw "IONOS bridge did not start: $audio" }
    }
    Start-Sleep -Seconds 5
    foreach ($process in $processes) {
        $process.Refresh()
        if ($process.HasExited) { throw "A $Modem process exited during setup: $($process.ExitCode)" }
    }
    if ($Modem -eq 'Mercury') {
        foreach ($index in 0,1) {
            $log = Join-Path $out "station-$index/mercury.log"
            if ((Test-Path -LiteralPath $log) -and
                (Select-String -LiteralPath $log -Quiet -Pattern '\[ERR\]\s+\[audio-(cap|play)\].*error in audio->open')) {
                throw "Mercury failed to open a WASAPI endpoint. Check the device ID and Windows desktop microphone access in $log"
            }
        }
    }
    if ($Reverse) {
        [array]::Reverse($ports)
        [array]::Reverse($calls)
    }
    $arqArgs = @((Join-Path $PSScriptRoot 'hf-arq-benchmark.py'), '--ports', $ports[0], $ports[1], '--output',
        (Join-Path $out 'arq'), '--channel', $Channel, '--modem', $Modem, '--bytes', $Bytes,
        '--calls', $calls[0], $calls[1], '--timeout', $Timeout, '--connect-timeout', $ConnectTimeout,
        '--payload-seed', $PayloadSeed)
    if ($IntegrityGrace -gt 0) { $arqArgs += @('--integrity-grace', $IntegrityGrace.ToString($invariant)) }
    if ($Aligned -and $bridge) { $arqArgs += @('--alignment', $audio) }
    & $Python @arqArgs
    $result = $LASTEXITCODE
} finally {
    if ($bridge) {
        $null = New-Item -ItemType File -Path (Join-Path $audio 'stop') -Force
        if (!$bridge.WaitForExit(8000)) { Stop-Process -InputObject $bridge }
    }
    foreach ($process in $processes) {
        $process.Refresh()
        if (!$process.HasExited) {
            Stop-Process -InputObject $process
            $null = $process.WaitForExit(5000)
        }
    }
    if ($Modem -eq 'VARA') {
        foreach ($index in 0,1) {
            $cleanup = @('VARA.ini')
            if ($DiscardVaraRuntime) { $cleanup += $varaFiles }
            foreach ($name in $cleanup) {
                $file = Join-Path $out "station-$index/$name"
                if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file }
            }
        }
    }
}
exit $result
