<#
.SYNOPSIS
  Builds llama.cpp (MIT), Trovara's embedding runtime, as DLLs in vendor\llama (TRD §15.1).

.DESCRIPTION
  The pinned tag and commit, plus third_party\patches\llama.cpp-no-logits.patch. ggml's backends are modules loaded at
  run time (GGML_BACKEND_DL): every CPU variant always (one build fits any x86-64 CPU), plus Vulkan when the Vulkan
  SDK is installed ($env:VULKAN_SDK) and CUDA with -Cuda (the optional CUDA pack). Runs inside the MSVC environment
  of the newest Visual Studio with C++ tools (found with vswhere), with its own CMake and Ninja.

  Output: vendor\llama\bin (DLLs), vendor\llama\lib (import libraries), vendor\llama\include (headers).
  Does nothing when that build is already there; -Force builds again.
#>
param(
    [switch]$Cuda,
    [switch]$NoVulkan,
    [switch]$Force
)
# 'Continue': under 'Stop', Windows PowerShell 5.1 aborts on any native tool's stderr (cmake warnings) when output is
# redirected. Every native step checks $LASTEXITCODE instead.
$ErrorActionPreference = 'Continue'
$Root = Split-Path -Parent $PSScriptRoot

$Tag = 'b11461'                                       # the first tags with EmbeddingGemma 2 (text + vision + audio) are b11452+
$Commit = '4d756bc72bf00a4aacf410ae15a2d315f3db400d'  # the tag's commit: a tag can be moved, a commit can't
# A new tag ships only if the reference-vector test passes (crates/trovara-llama). If it fails, keep this tag, or bump
# trovara_llama::VECTOR_VERSION and write the reference vectors again: every index then embeds again.
$Patch = Join-Path $Root 'third_party\patches\llama.cpp-no-logits.patch'
$Source = if ($env:LLAMA_SOURCE) { $env:LLAMA_SOURCE } else { Join-Path $Root 'third_party\llama.cpp' }
$Build = if ($env:LLAMA_BUILD) { $env:LLAMA_BUILD } else { Join-Path $Root 'target\llama-build' }
$Out = Join-Path $Root 'vendor\llama'

# A terminal opened before the SDK was installed doesn't have VULKAN_SDK yet: take it from the system settings.
if (-not $env:VULKAN_SDK) {
    $env:VULKAN_SDK = [Environment]::GetEnvironmentVariable('VULKAN_SDK', 'Machine')
}
if ($env:VULKAN_SDK) { $env:Path = "$env:VULKAN_SDK\Bin;$env:Path" }
$Flavor = if ($Cuda) { 'cuda' } elseif (-not $NoVulkan -and $env:VULKAN_SDK) { 'vulkan' } else { 'cpu' }
$PatchHash = (Get-FileHash $Patch -Algorithm SHA256).Hash.Substring(0, 12).ToLower()
$Stamp = "$Tag $PatchHash $Flavor"
$StampFile = Join-Path $Out 'BUILD'
if (-not $Force -and (Test-Path (Join-Path $Out 'bin\llama.dll')) -and (Test-Path $StampFile) -and
    ((Get-Content $StampFile -Raw).Trim() -eq $Stamp)) {
    Write-Host "llama.cpp $Tag ($Flavor) is already built in $Out"
    exit 0
}

# The MSVC environment (cl, link, the Windows SDK, Visual Studio's CMake and Ninja), imported into this session.
$VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $VsWhere)) { throw 'Visual Studio is not installed (vswhere.exe not found).' }
$Vs = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $Vs) { throw 'No Visual Studio with the C++ x64 tools ("Desktop development with C++") was found.' }
$VcVars = Join-Path $Vs 'VC\Auxiliary\Build\vcvars64.bat'
$env:Path = "$(Split-Path $VsWhere);$env:Path"   # vcvars itself calls vswhere
cmd /c "`"$VcVars`" > nul && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
}
# Visual Studio's own CMake and Ninja first: others on PATH (MSYS2, Strawberry Perl) may target MinGW.
$VsCMake = Join-Path $Vs 'Common7\IDE\CommonExtensions\Microsoft\CMake'
$env:Path = "$VsCMake\CMake\bin;$VsCMake\Ninja;$env:Path"
foreach ($tool in 'cl', 'cmake', 'ninja', 'git') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool not found in the MSVC environment" }
}
Write-Host "MSVC: $Vs"
Write-Host "CMake: $((Get-Command cmake).Source)"

# The source at the pinned commit, with line endings as they are (else the patch wouldn't apply).
if (-not (Test-Path $Source)) {
    git -c advice.detachedHead=false -c core.autocrlf=false clone --quiet --depth 1 --branch $Tag `
        https://github.com/ggml-org/llama.cpp $Source
    if ($LASTEXITCODE) { throw 'git clone failed' }
} elseif ((git -C $Source describe --tags --exact-match 2>$null) -ne $Tag) {
    git -C $Source fetch --quiet --depth 1 origin tag $Tag
    git -C $Source checkout --quiet --force $Tag   # --force also drops the old patch
    if ($LASTEXITCODE) { throw "git checkout $Tag failed" }
}
git -C $Source config core.autocrlf false
$Head = (git -C $Source rev-parse HEAD).Trim()
if ($Head -ne $Commit) { throw "llama.cpp's $Tag is commit $Head here, not ${Commit}: not building it" }

# Without the patch, every input token costs 1 MiB of logits that an embedding model never produces (262k vocabulary).
# (Through cmd: Windows PowerShell turns a native command's stderr into a terminating error.)
cmd /c "git -C `"$Source`" apply --check `"$Patch`" 2>nul"
if ($LASTEXITCODE -eq 0) {
    git -C $Source apply $Patch
    if ($LASTEXITCODE) { throw 'git apply failed' }
} else {
    cmd /c "git -C `"$Source`" apply --reverse --check `"$Patch`" 2>nul"
    if ($LASTEXITCODE) { throw "The patch doesn't apply to llama.cpp $Tag" }   # else it's already applied
}

$Gpu = @(switch ($Flavor) {
    'cuda' { @('-DGGML_CUDA=ON', "-DCMAKE_CUDA_ARCHITECTURES=$(if ($env:CUDA_ARCHS) { $env:CUDA_ARCHS } else { 'native' })") }
    'vulkan' { @('-DGGML_VULKAN=ON') }
    default { @() }
})   # @(...): a one-item result would otherwise be a string, splatted character by character
Write-Host "Configuring llama.cpp $Tag ($Flavor)..."
# A build folder configured for another flavor keeps its old cache: start clean when forced or when the flavor changed.
$FlavorFile = Join-Path $Build 'FLAVOR'
if ($Force -or ((Test-Path $FlavorFile) -and ((Get-Content $FlavorFile -Raw).Trim() -ne $Flavor))) {
    Remove-Item -Recurse -Force $Build -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Force $Build | Out-Null
Set-Content -Path $FlavorFile -Value $Flavor -Encoding ascii
$ConfigureLog = Join-Path $Build 'configure.log'
cmake -S $Source -B $Build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_COMPILER=cl -DCMAKE_CXX_COMPILER=cl `
    -DBUILD_SHARED_LIBS=ON -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON -DGGML_NATIVE=OFF @Gpu `
    -DLLAMA_BUILD_MTMD=ON -DLLAMA_BUILD_COMMON=OFF -DLLAMA_BUILD_TOOLS=OFF -DLLAMA_BUILD_SERVER=OFF `
    -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_APP=OFF -DLLAMA_BUILD_UI=OFF `
    -DLLAMA_OPENSSL=OFF -DLLAMA_CURL=OFF *> $ConfigureLog
if ($LASTEXITCODE) {
    Get-Content $ConfigureLog -Tail 40
    throw "cmake configure failed: see $ConfigureLog"
}
Write-Host 'Building (a few minutes)...'
cmake --build $Build --config Release -j $env:NUMBER_OF_PROCESSORS
if ($LASTEXITCODE) { throw 'cmake build failed' }

# The DLLs (the backends among them), the import libraries the helper links against, and the headers.
Remove-Item -Recurse -Force $Out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $Out 'bin'), (Join-Path $Out 'lib'), (Join-Path $Out 'include') | Out-Null
Copy-Item (Join-Path $Build 'bin\*.dll') (Join-Path $Out 'bin')
Get-ChildItem $Build -Recurse -Include llama.lib, mtmd.lib, ggml.lib, ggml-base.lib |
    Copy-Item -Destination (Join-Path $Out 'lib')
Copy-Item (Join-Path $Source 'include\llama.h'), (Join-Path $Source 'tools\mtmd\mtmd.h'),
    (Join-Path $Source 'tools\mtmd\mtmd-helper.h') (Join-Path $Out 'include')
Copy-Item (Join-Path $Source 'ggml\include\*.h') (Join-Path $Out 'include')
Set-Content -Path $StampFile -Value $Stamp -Encoding ascii

$Dlls = Get-ChildItem (Join-Path $Out 'bin\*.dll')
$Mb = [math]::Round((($Dlls | Measure-Object Length -Sum).Sum / 1MB), 1)
Write-Host "Built llama.cpp $Tag (patched, $Flavor) into $Out : $($Dlls.Count) DLLs, $Mb MB"
