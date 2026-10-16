# Builds Trovara's installer (TRD §15): the helpers, the DLLs they need and the Visual C++ runtime, staged in
# target\package and signed (scripts\sign.ps1, when a certificate is set up); then the app and the NSIS installer
# (Tauri), the updater's signature, and latest.json for the update feed.
#
#   powershell -File scripts\release.ps1 [-UpdateUrl <latest.json URL>] [-DownloadBase <URL of the release's files>]
#                                         [-Notes "What's new"]
#
#   -UpdateUrl     where the app looks for updates, baked into this build (TROVARA_UPDATE_URL). GitHub Releases:
#                  https://github.com/<owner>/<repo>/releases/latest/download/latest.json. Without it the build never
#                  checks for updates.
#   -DownloadBase  where this release's installer will be downloadable (for latest.json). GitHub Releases:
#                  https://github.com/<owner>/<repo>/releases/download/v<version>
#
# The update signing key: %USERPROFILE%\.trovara\updater.key (+ updater.password), made once with
# `npx tauri signer generate`. Never commit it: whoever has it can ship updates to every Trovara.

#   Update tests only: -Version 0.1.1 builds as another version; -AllowHttp lets the feed be plain http (a
#   local server). Never ship a build made with -AllowHttp.
param(
    [string]$UpdateUrl = "",
    [string]$DownloadBase = "",
    [string]$Notes = "",
    [string]$Version = "",
    [switch]$AllowHttp
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$release = Join-Path $root "target\release"
$package = Join-Path $root "target\package"
$app = Join-Path $root "app"
$expectedCli = "2.12.1"   # the CLI app\src-tauri\nsis\installer.nsi was taken from

function Step([string]$text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }
# JSON without a byte-order mark (Windows PowerShell's utf8 adds one, which JSON readers reject).
function WriteJson([string]$path, [string]$text) {
    [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}
function Run([string]$what, [scriptblock]$block) {
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed ($LASTEXITCODE)" }
}

Step "Tauri CLI"
Push-Location $app
try {
    if (-not (Test-Path "node_modules\@tauri-apps\cli")) { Run "npm install" { npm install --no-audit --no-fund } }
    $cli = (npx tauri --version) -replace "tauri-cli ", ""
} finally { Pop-Location }
if ($cli -ne $expectedCli) {
    Write-Warning "Tauri CLI $cli, but the NSIS template is $expectedCli's: take the new template (see installer.nsi's header)"
}

Step "Helpers (release)"
if ($UpdateUrl) { $env:TROVARA_UPDATE_URL = $UpdateUrl } else { Remove-Item Env:TROVARA_UPDATE_URL -ErrorAction SilentlyContinue }
Push-Location $root
try { Run "cargo build" { cargo build --release --locked -p trovara-helper -p trovara-usn } } finally { Pop-Location }

Step "Staging target\package"
if (Test-Path $package) { Remove-Item -Recurse -Force $package }
New-Item -ItemType Directory $package | Out-Null
Copy-Item (Join-Path $release "trovara-helper.exe"), (Join-Path $release "trovara-usn.exe") $package
Copy-Item (Join-Path $release "*.dll") $package
# The Visual C++ runtime, app-local (TRD §20.3): msvcp140, vcruntime140(_1) and vcomp140 (ggml's OpenMP).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -property installationPath
$redist = Get-ChildItem (Join-Path $vs "VC\Redist\MSVC") -Directory | Where-Object { $_.Name -match '^\d' } |
    Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
$crt = Get-ChildItem (Join-Path $redist.FullName "x64") -Directory -Filter "Microsoft.VC*.CRT" | Select-Object -First 1
$openmp = Get-ChildItem (Join-Path $redist.FullName "x64") -Directory -Filter "Microsoft.VC*.OpenMP" | Select-Object -First 1
foreach ($dll in "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll") { Copy-Item (Join-Path $crt.FullName $dll) $package }
Copy-Item (Join-Path $openmp.FullName "vcomp140.dll") $package
Write-Host "VC++ runtime $($redist.Name)"
& (Join-Path $PSScriptRoot "acknowledgements.ps1") -Out (Join-Path $package "Acknowledgements.txt")
Copy-Item (Join-Path $package "Acknowledgements.txt") (Join-Path $root "app\src-tauri\resources\Acknowledgements.txt") -Force

Step "Signing the staged programs and DLLs"
$signer = Join-Path $PSScriptRoot "sign.ps1"
$pe = Get-ChildItem $package -File | Where-Object { $_.Extension -in ".exe", ".dll" } |
    Where-Object { $_.Name -notmatch '^(msvcp|vcruntime|vcomp)' }   # Microsoft's own are signed by Microsoft
& $signer @($pe.FullName)

Step "App and installer (tauri build)"
$overrides = @{}
$certificate = $env:TROVARA_SIGN_PFX -or $env:TROVARA_SIGN_THUMBPRINT -or $env:TROVARA_SIGN_DLIB
if ($certificate) {
    # Tauri signs trovara.exe, the uninstaller and the installer with the same script.
    $overrides.bundle = @{ windows = @{ signCommand = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$signer`" %1" } }
}
$key = Join-Path $env:USERPROFILE ".trovara\updater.key"
if (Test-Path $key) {
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $key -Raw
    $passwordFile = Join-Path $env:USERPROFILE ".trovara\updater.password"
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = if (Test-Path $passwordFile) { Get-Content $passwordFile -Raw } else { "" }
} else {
    Write-Warning "no update signing key ($key): the installer is built without an update signature"
    if (-not $overrides.bundle) { $overrides.bundle = @{} }
    $overrides.bundle.createUpdaterArtifacts = $false
}
if ($Version) { $overrides.version = $Version }
if ($AllowHttp) { $overrides.plugins = @{ updater = @{ dangerousInsecureTransportProtocol = $true } } }
$configArgs = @()
if ($overrides.Count) {
    $configFile = Join-Path $root "target\tauri-release.json"
    WriteJson $configFile ($overrides | ConvertTo-Json -Depth 6)
    $configArgs = @("--config", $configFile)
}
Push-Location $app
try { Run "tauri build" { npx tauri build @configArgs } } finally {
    Pop-Location
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}

Step "Result"
$bundle = Join-Path $release "bundle\nsis"
$installer = Get-ChildItem $bundle -Filter "*-setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
# Named for people: Trovara-Setup-<version>.exe (Tauri's is Trovara_<version>_x64-setup.exe). The update signature
# covers the file's bytes, not its name, so it's renamed with it; latest.json below uses the new name.
$version = if ($Version) { $Version } else { (Get-Content (Join-Path $app "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version }
$named = Join-Path $bundle "Trovara-Setup-$version.exe"
if ($installer.FullName -ne $named) {
    Move-Item -Force $installer.FullName $named
    if (Test-Path "$($installer.FullName).sig") { Move-Item -Force "$($installer.FullName).sig" "$named.sig" }
    $installer = Get-Item $named
}
$megabytes = [math]::Round($installer.Length / 1MB, 1)
$budget = if ($megabytes -gt 60) { " (over the 60 MB budget, NFR-06)" } else { " (budget 60 MB)" }
Write-Host "$($installer.FullName): $megabytes MB$budget"
$signature = "$($installer.FullName).sig"
if ($UpdateUrl -and (Test-Path $signature)) {
    if (-not $DownloadBase) { throw "-DownloadBase is needed with -UpdateUrl (where the installer will be downloadable)" }
    $feed = [ordered]@{
        version = $version
        notes = $Notes
        pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = [ordered]@{
            "windows-x86_64" = [ordered]@{
                signature = (Get-Content $signature -Raw).Trim()
                url = "$($DownloadBase.TrimEnd('/'))/$([uri]::EscapeDataString($installer.Name))"
            }
        }
    }
    $feedFile = Join-Path $bundle "latest.json"
    WriteJson $feedFile ($feed | ConvertTo-Json -Depth 5)
    Write-Host "update feed: $feedFile (upload it with the installer)"
}
