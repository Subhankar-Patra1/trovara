# Signs Windows programs and DLLs with Authenticode (TRD §15.3), when a certificate is set up; otherwise says so and
# leaves the files as they are (an unsigned beta: SmartScreen warns until signed builds earn reputation).
#
#   powershell -File scripts\sign.ps1 <file>...
#
# Where the certificate comes from, first match:
#   TROVARA_SIGN_PFX (+ TROVARA_SIGN_PASSWORD)   a .pfx file
#   TROVARA_SIGN_THUMBPRINT                      a certificate in the user's store (a hardware token's included)
#   TROVARA_SIGN_DLIB + TROVARA_SIGN_METADATA     Azure Trusted Signing (its dlib and metadata.json)
# TROVARA_SIGN_TIMESTAMP: the RFC 3161 time-stamp server (default DigiCert's).

param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Files)

$ErrorActionPreference = "Stop"
if (-not $Files) { throw "sign which files?" }

$how = @()
if ($env:TROVARA_SIGN_PFX) {
    $how = @("/f", $env:TROVARA_SIGN_PFX)
    if ($env:TROVARA_SIGN_PASSWORD) { $how += @("/p", $env:TROVARA_SIGN_PASSWORD) }
} elseif ($env:TROVARA_SIGN_THUMBPRINT) {
    $how = @("/sha1", $env:TROVARA_SIGN_THUMBPRINT)
} elseif ($env:TROVARA_SIGN_DLIB -and $env:TROVARA_SIGN_METADATA) {
    $how = @("/dlib", $env:TROVARA_SIGN_DLIB, "/dmdf", $env:TROVARA_SIGN_METADATA)
} else {
    Write-Host "sign: no certificate set up (TROVARA_SIGN_PFX, TROVARA_SIGN_THUMBPRINT or TROVARA_SIGN_DLIB): left unsigned"
    exit 0
}

# The newest signtool from the Windows SDK.
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object { [version]($_.Directory.Parent.Name) } -Descending | Select-Object -First 1
if (-not $signtool) { throw "signtool.exe not found: install the Windows SDK" }
$timestamp = if ($env:TROVARA_SIGN_TIMESTAMP) { $env:TROVARA_SIGN_TIMESTAMP } else { "http://timestamp.digicert.com" }

foreach ($file in $Files) {
    & $signtool.FullName sign /fd sha256 /tr $timestamp /td sha256 @how $file
    if ($LASTEXITCODE -ne 0) { throw "signing $file failed" }
}
