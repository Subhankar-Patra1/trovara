<#
.SYNOPSIS
  Downloads the prebuilt libraries Trovara ships beside its helper, at pinned versions, into vendor\ (TRD §15.1).

.DESCRIPTION
  PDFium (PDF text and page renders), prebuilt by bblanchon/pdfium-binaries. Does nothing when the pinned set is
  already there.
#>
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$PdfiumTag = 'chromium/8086'
$Stamp = "pdfium $PdfiumTag"
$Vendor = Join-Path $Root 'vendor'
$StampFile = Join-Path $Vendor 'VENDOR'
if ((Test-Path $StampFile) -and ((Get-Content $StampFile -Raw).Trim() -eq $Stamp)) {
    Write-Host 'vendor\ is up to date'
    exit 0
}

$Temp = Join-Path $Root 'target\fetch-tmp'
Remove-Item -Recurse -Force $Temp -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Temp | Out-Null

Write-Host "PDFium ($PdfiumTag, win-x64)"
$Url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$([uri]::EscapeDataString($PdfiumTag))/pdfium-win-x64.tgz"
$Archive = Join-Path $Temp 'pdfium.tgz'
Invoke-WebRequest -Uri $Url -OutFile $Archive -UseBasicParsing
$Pdfium = Join-Path $Vendor 'pdfium'
Remove-Item -Recurse -Force $Pdfium -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Pdfium | Out-Null
tar -xzf $Archive -C $Pdfium   # Windows 10+ ships bsdtar
if ($LASTEXITCODE) { throw 'tar failed' }

Set-Content -Path $StampFile -Value $Stamp -Encoding ascii
Remove-Item -Recurse -Force $Temp
Write-Host "Done: $Pdfium"
