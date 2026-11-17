param(
    [string]$IonSource = "$PSScriptRoot/../run/hf-benchmark-sources/ion-hf",
    [string]$Output = "$PSScriptRoot/../run/hf-benchmark-sources"
)

$ErrorActionPreference = 'Stop'
$ionCommit = '1eb7dca01803e120eeb0924fd02d4ccc563557a4'
$IonSource = (Resolve-Path -LiteralPath $IonSource).Path
$Output = [IO.Path]::GetFullPath($Output)
$null = Get-Command cl.exe, rustc, git
if ((git -C $IonSource rev-parse HEAD) -ne $ionCommit) {
    throw "Check out Ion HF $ionCommit before building the benchmark tools."
}
git -C $IonSource diff --quiet HEAD -- crates/ion-dsp/src
if ($LASTEXITCODE -ne 0) { throw 'The pinned ion-dsp sources have local changes.' }

$null = New-Item -ItemType Directory -Path $Output -Force
$dsp = Join-Path $IonSource 'crates/ion-dsp'
$example = Join-Path $Output 'ionos_pipe_48k_gain.rs'
$library = Join-Path $Output 'libion_dsp.rlib'

# Adapt Ion HF's Apache-2.0 example for WASAPI's 48 kHz stream and calibrated output gain.
$source = (git -C $IonSource show 'HEAD:crates/ion-dsp/examples/ionos_pipe.rs') -join "`n"
if ($LASTEXITCODE -ne 0) { throw 'Could not read the pinned IONOS pipe example.' }
$source = $source.Replace('const RATE: u32 = 8_000;', 'const RATE: u32 = 48_000;').
    Replace('const DELAY: usize = 320;', 'const DELAY: usize = 1920;').
    Replace('    let snr: Option<f32>', "    let output_gain: f32 = arg(&args, `"--output-gain`").unwrap_or(1.0);`n    let snr: Option<f32>").
    Replace('(v * v) as f64).sum', '(v * v * output_gain * output_gain) as f64).sum').
    Replace('(v * 32_768.0).round()', '(v * output_gain * 32_768.0).round()')
[IO.File]::WriteAllText($example, $source)

& rustc --edition=2024 --crate-type lib --crate-name ion_dsp -O (Join-Path $dsp 'src/lib.rs') -o $library
if ($LASTEXITCODE -ne 0) { throw 'Could not compile ion-dsp.' }
& rustc --edition=2024 -O $example --extern "ion_dsp=$library" -o (Join-Path $Output 'ionos_pipe_48k_gain.exe')
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the 48 kHz IONOS pipe.' }

$object = Join-Path $Output 'hf-process-audio.obj'
$capture = Join-Path $Output 'hf-process-audio.exe'
& cl.exe /nologo /O2 /EHsc /std:c++17 "/Fo$object" "/Fe$capture" (Join-Path $PSScriptRoot 'hf-process-audio.cpp') /link mmdevapi.lib ole32.lib
if ($LASTEXITCODE -ne 0) { throw 'Could not compile the Windows process loopback helper.' }
