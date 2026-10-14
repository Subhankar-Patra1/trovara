# Writes Acknowledgements.txt (PRD §6.12 About → Licenses, TRD §15): the notices of everything Trovara ships.
#   - Trovara's own third-party notices (THIRD_PARTY_NOTICES.md)
#   - every Rust crate compiled into the Windows binaries (normal dependencies, not dev or build ones), with the
#     license files each crate ships; one copy of each distinct text, followed by the crates that use it
#   - llama.cpp and ggml, PDFium (and the licenses PDFium lists), the Microsoft Visual C++ runtime, the model's terms
#
#   powershell -File scripts\acknowledgements.ps1 [-Out app\src-tauri\resources\Acknowledgements.txt]

param(
    [string]$Out = (Join-Path $PSScriptRoot "..\app\src-tauri\resources\Acknowledgements.txt")
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Push-Location $root
try {
    $json = cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
    $meta = $json | ConvertFrom-Json
} finally {
    Pop-Location
}

# What ships: the workspace members' normal dependencies, all the way down.
$nodes = @{}
foreach ($node in $meta.resolve.nodes) { $nodes[$node.id] = $node }
$members = @{}
foreach ($id in $meta.workspace_members) { $members[$id] = $true }
$shipped = @{}
$queue = New-Object System.Collections.Queue
foreach ($id in $meta.workspace_members) { $queue.Enqueue($id) }
while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    foreach ($dep in $nodes[$id].deps) {
        $normal = @($dep.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0
        if ($normal -and -not $shipped.ContainsKey($dep.pkg)) {
            $shipped[$dep.pkg] = $true
            $queue.Enqueue($dep.pkg)
        }
    }
}
$packages = $meta.packages | Where-Object { $shipped.ContainsKey($_.id) -and -not $members.ContainsKey($_.id) } |
    Sort-Object name, version

# Each distinct license text once, with the crates that ship it.
$texts = [ordered]@{}      # hash → text
$users = @{}               # hash → list of "name version"
$bare = New-Object System.Collections.Generic.List[string]   # crates with no license file: their SPDX expression
$sha = [System.Security.Cryptography.SHA256]::Create()
foreach ($package in $packages) {
    $folder = Split-Path $package.manifest_path
    $files = Get-ChildItem -LiteralPath $folder -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)([-._].*)?$' } | Sort-Object Name
    $label = "$($package.name) $($package.version)" + $(if ($package.license) { " ($($package.license))" } else { "" })
    if (-not $files) {
        $bare.Add($label)
        continue
    }
    foreach ($file in $files) {
        $text = ([System.IO.File]::ReadAllText($file.FullName)).Replace("`r`n", "`n").Trim()
        $hash = [BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($text)))
        if (-not $texts.Contains($hash)) {
            $texts[$hash] = $text
            $users[$hash] = New-Object System.Collections.Generic.List[string]
        }
        if (-not $users[$hash].Contains($label)) { $users[$hash].Add($label) }
    }
}

$line = "=" * 100
$doc = New-Object System.Text.StringBuilder
function Add([string]$text) { [void]$doc.Append($text.Replace("`r`n", "`n")).Append("`n") }
function Section([string]$title) { Add ""; Add $line; Add $title; Add $line; Add "" }
function AddFile([string]$path) { if (Test-Path -LiteralPath $path) { Add ([System.IO.File]::ReadAllText($path).Trim()) } }

$version = (Get-Content (Join-Path $root "app\src-tauri\Cargo.toml") | Select-String '^version = "(.+)"').Matches[0].Groups[1].Value
Add "Trovara ${version}: acknowledgements"
Add ""
Add "Trovara is made with the work of many people. Their notices and licenses follow: Trovara's own third-party"
Add "notices, the native components it ships (llama.cpp and ggml, PDFium, the Visual C++ runtime), the model's terms,"
Add "and the $($packages.Count) Rust crates compiled into it."

Section "Trovara's third-party notices"
AddFile (Join-Path $root "THIRD_PARTY_NOTICES.md")

Section "llama.cpp and ggml (MIT), https://github.com/ggml-org/llama.cpp"
AddFile (Join-Path $root "third_party\llama.cpp\LICENSE")
$llamaLicenses = Join-Path $root "third_party\llama.cpp\licenses"
if (Test-Path $llamaLicenses) {
    foreach ($file in Get-ChildItem $llamaLicenses -File | Sort-Object Name) {
        Add ""; Add "--- llama.cpp: $($file.Name)"; Add ""; AddFile $file.FullName
    }
}

Section "PDFium (BSD-3-Clause and others), https://pdfium.googlesource.com/pdfium"
AddFile (Join-Path $root "vendor\pdfium\LICENSE")
$pdfiumLicenses = Join-Path $root "vendor\pdfium\licenses"
if (Test-Path $pdfiumLicenses) {
    foreach ($file in Get-ChildItem $pdfiumLicenses -File | Sort-Object Name) {
        Add ""; Add "--- PDFium: $($file.Name)"; Add ""; AddFile $file.FullName
    }
}

Section "Microsoft Visual C++ runtime"
Add "msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll and vcomp140.dll are Microsoft Visual C++ Redistributable files,"
Add "shipped with Trovara under the Visual Studio license terms for distributable code."

Section "EmbeddingGemma 2 (the model, downloaded on first launch, not part of the installer)"
Add "Trovara downloads google/embeddinggemma-2 (ggml-org's GGUF conversion) from Hugging Face. It is provided by"
Add "Google under the Gemma Terms of Use, https://ai.google.dev/gemma/terms, and its Prohibited Use Policy,"
Add "https://ai.google.dev/gemma/prohibited_use_policy."

Section "Rust crates"
foreach ($hash in $texts.Keys) {
    Add ("Used by: " + ($users[$hash] -join ", "))
    Add ""
    Add $texts[$hash]
    Add ""
    Add ("-" * 100)
    Add ""
}
if ($bare.Count -gt 0) {
    Add "These crates ship no license file; their license, as each declares it:"
    foreach ($label in $bare) { Add "  $label" }
}

$folder = Split-Path $Out
if (-not (Test-Path $folder)) { New-Item -ItemType Directory -Force $folder | Out-Null }
[System.IO.File]::WriteAllText($Out, $doc.ToString(), (New-Object System.Text.UTF8Encoding($false)))
"{0}: {1} crates, {2} license texts, {3:N0} KB" -f $Out, $packages.Count, $texts.Count, ((Get-Item $Out).Length / 1KB)
