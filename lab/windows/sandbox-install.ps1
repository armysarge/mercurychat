# M5 Task 12: the NSIS installer's clean install, inside Windows Sandbox
# (sandbox-install.wsb). The same checks as lab/release/clean-install.sh:
#   I1 the installer installs per user, silently, with no network
#   I2 modem\mercury.exe -h and modem\rigctld.exe --version run
#   I3 mercurychat.exe --version runs and names the versions
#   I4 the app reports "bundle: verified"; MANIFEST.sha256 checks
#   I5 upgrade_keeps_data: install N, a marker in the data folder, install
#      N+1 over it, the marker is still there
# Writes C:\mc\out\result.txt (RESULT PASS / RESULT FAIL) and shuts the sandbox down.
$ErrorActionPreference = "Continue"
$out = "C:\mc\out\result.txt"
$lines = New-Object System.Collections.Generic.List[string]
$fails = 0
function Pass($m) { $lines.Add("PASS $m") }
function Fail($m) { $lines.Add("FAIL $m"); $script:fails++ }
function Run-Capture($exe, $argList) {
  $o = [IO.Path]::GetTempFileName(); $e = [IO.Path]::GetTempFileName()
  $p = Start-Process -FilePath $exe -ArgumentList $argList -Wait -PassThru -NoNewWindow -RedirectStandardOutput $o -RedirectStandardError $e
  [pscustomobject]@{ Code = $p.ExitCode; Text = ((Get-Content $o -Raw) + (Get-Content $e -Raw)) }
}

$setup = Get-ChildItem C:\mc\in -Filter "MercuryChat_*-setup.exe" | Select-Object -First 1
if (-not $setup) { "FAIL no MercuryChat_*-setup.exe in the in folder`nRESULT FAIL" | Set-Content $out; Stop-Computer -Force; exit 1 }
$older = Get-Item C:\mc\in\older-setup.exe -ErrorAction SilentlyContinue
if (-not $older) { $older = $setup; $lines.Add("note: no older-setup.exe; N is the same installer (reinstall over itself)") }
$lines.Add("== $($setup.Name) on $((Get-CimInstance Win32_OperatingSystem).Caption)")

$app = Join-Path $env:LOCALAPPDATA "MercuryChat"
$data = Join-Path $env:APPDATA "org.mercurychat.desktop"

# I5 part 1: N, then a marker.
$n = Start-Process $older.FullName -ArgumentList "/S" -Wait -PassThru
if ($n.ExitCode -eq 0 -and (Test-Path "$app\mercurychat.exe")) {
  New-Item -ItemType Directory -Force $data | Out-Null
  "kept $(Get-Date -Format o)" | Set-Content "$data\upgrade-marker.txt"
} else { Fail "I5 the older installer N did not install (exit $($n.ExitCode))" }

# I1: N+1 over it.
$p = Start-Process $setup.FullName -ArgumentList "/S" -Wait -PassThru
if ($p.ExitCode -eq 0 -and (Test-Path "$app\mercurychat.exe")) { Pass "I1 installed per user in $app" } else { Fail "I1 install (exit $($p.ExitCode)); $app\mercurychat.exe missing" }

# I2: the modem's programs.
$m = Run-Capture "$app\modem\mercury.exe" "-h"
if ($m.Text.Trim().Length -gt 0 -and $m.Code -ne -1073741515) { Pass "I2 mercury.exe -h runs (exit $($m.Code))" } else { Fail "I2 mercury.exe -h (exit $($m.Code)): $($m.Text)" }
$r = Run-Capture "$app\modem\rigctld.exe" "--version"
if ($r.Code -eq 0) { Pass "I2 rigctld --version: $(($r.Text -split "`n")[0])" } else { Fail "I2 rigctld --version (exit $($r.Code)): $($r.Text)" }

# I3, I4: the app and the manifest.
$v = Run-Capture "$app\mercurychat.exe" "--version"
if ($v.Code -eq 0 -and $v.Text -match "^MercuryChat ") { Pass "I3 mercurychat --version: $($v.Text -replace "`r?`n", ' | ')" } else { Fail "I3 mercurychat --version (exit $($v.Code)): $($v.Text)" }
if ($v.Text -match "(?m)^bundle: verified") { Pass "I4 the app verifies its bundle" } else { Fail "I4 no 'bundle: verified'" }
$bad = 0
foreach ($l in Get-Content "$app\modem\MANIFEST.sha256") {
  $sha, $name = $l -split "  ", 2
  if ((Get-FileHash "$app\modem\$name" -Algorithm SHA256).Hash.ToLower() -ne $sha) { $bad++ }
}
if ($bad -eq 0) { Pass "I4 MANIFEST.sha256 checks" } else { Fail "I4 $bad file(s) differ from MANIFEST.sha256" }

# I5 part 2.
if (Test-Path "$data\upgrade-marker.txt") { Pass "I5 upgrade_keeps_data: marker kept ($(Get-Content "$data\upgrade-marker.txt"))" } else { Fail "I5 upgrade_keeps_data: the marker is gone" }

$lines.Add($(if ($fails -eq 0) { "RESULT PASS" } else { "RESULT FAIL ($fails)" }))
$lines | Set-Content $out
Stop-Computer -Force
