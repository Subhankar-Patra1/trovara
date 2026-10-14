# Generate icon assets from the approved logo. This script does not build the app.
$ErrorActionPreference = "Stop"
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$appPath = Join-Path $projectRoot "app"
$sourcePath = Join-Path $appPath "branding\icon-source.png"
$cliPath = Join-Path $appPath "node_modules\.bin\tauri.cmd"
$outputPath = Join-Path $projectRoot "target\brand-icons"
$iconsPath = Join-Path $appPath "src-tauri\icons"

if (-not (Test-Path -LiteralPath $sourcePath)) { throw "Logo source is missing: $sourcePath" }
if (-not (Test-Path -LiteralPath $cliPath)) { throw "Install the dependencies in app before generating icons." }

Push-Location $appPath
try {
    & $cliPath icon $sourcePath --output $outputPath
    if ($LASTEXITCODE -ne 0) { throw "Icon generation failed ($LASTEXITCODE)." }
} finally { Pop-Location }

foreach ($name in @("32x32.png", "128x128.png", "128x128@2x.png", "icon.png", "icon.ico")) {
    Copy-Item -LiteralPath (Join-Path $outputPath $name) -Destination (Join-Path $iconsPath $name)
}
Copy-Item -LiteralPath (Join-Path $outputPath "32x32.png") -Destination (Join-Path $iconsPath "tray.png")
Copy-Item -LiteralPath (Join-Path $outputPath "128x128@2x.png") -Destination (Join-Path $appPath "ui\icon.png")
Copy-Item -LiteralPath (Join-Path $outputPath "icon.png") -Destination (Join-Path $appPath "ui\logo.png")
Write-Host "Updated app, tray, setup, Settings, and installer icon assets."
