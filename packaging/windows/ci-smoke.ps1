$ErrorActionPreference = "Stop"

$iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) {
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup installation failed: $LASTEXITCODE" }
}
if (-not (Test-Path $iscc)) { throw "Inno Setup compiler is unavailable" }

$classes = "Registry::HKEY_CURRENT_USER\Software\Classes"
$owned = @(
    "$classes\RustZIP.Archive",
    "$classes\Applications\rustzip.exe",
    "$classes\*\shell\RustZIP.Compress",
    "$classes\Directory\shell\RustZIP.Compress",
    "$classes\SystemFileAssociations\.zip\shell\RustZIP.Extract"
)
foreach ($key in $owned) {
    if (Test-Path -LiteralPath $key) { throw "Smoke test requires a clean account: $key" }
}

$work = Join-Path $env:RUNNER_TEMP ("rustzip-installer-" + [guid]::NewGuid().ToString("N"))
$install = Join-Path $work "app"
$dist = Join-Path $work "dist"
New-Item -ItemType Directory -Path $dist | Out-Null
$exe = Join-Path $PWD "target\release\rustzip.exe"
$zipKey = "$classes\.zip"
$zipBefore = if (Test-Path -LiteralPath $zipKey) { (Get-Item -LiteralPath $zipKey).GetValue("") } else { $null }

cargo build --release -p rustzip-gui
if ($LASTEXITCODE -ne 0) { throw "Windows GUI build failed: $LASTEXITCODE" }
& $iscc "/DAppVersion=0.1.0" "/DSourceExe=$exe" "/DOutDir=$dist" "packaging\windows\rustzip.iss"
if ($LASTEXITCODE -ne 0) { throw "Windows installer compilation failed: $LASTEXITCODE" }
$installer = Join-Path $dist "RustZIP-0.1.0-windows-x86_64.exe"
if (-not (Test-Path $installer)) { throw "Windows installer was not created" }

try {
    $process = Start-Process -FilePath $installer -ArgumentList @(
        "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART",
        "/DIR=`"$install`"", "/TASKS=contextmenu,openwith", "/LOG=`"$work\install.log`""
    ) -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
    foreach ($file in @("rustzip.exe", "z.dll", "bz2.dll", "liblzma.dll", "zstd.dll", "lz4.dll", "licenses\zlib.txt")) {
        if (-not (Test-Path -LiteralPath (Join-Path $install $file))) {
            throw "Installed file missing: $file"
        }
    }
    if (-not (Get-ChildItem -Path $install -Filter "libcrypto-*.dll")) {
        throw "OpenSSL runtime DLL is missing"
    }
    foreach ($key in $owned) {
        if (-not (Test-Path -LiteralPath $key)) { throw "Expected registry key missing: $key" }
    }
    $zipOpen = Get-Item -LiteralPath "$classes\.zip\OpenWithProgids"
    if ($zipOpen.GetValueNames() -notcontains "RustZIP.Archive") {
        throw "ZIP Open With registration is missing"
    }
    $rarOpen = Get-Item -LiteralPath "$classes\.rar\OpenWithProgids" -ErrorAction SilentlyContinue
    if ($null -ne $rarOpen -and $rarOpen.GetValueNames() -contains "RustZIP.Archive") {
        throw "Default build must not register RAR"
    }
    $zipAfter = if (Test-Path -LiteralPath $zipKey) { (Get-Item -LiteralPath $zipKey).GetValue("") } else { $null }
    if ($zipAfter -ne $zipBefore) { throw "Installation changed the ZIP default handler" }
} finally {
    $uninstaller = Join-Path $install "unins000.exe"
    if (Test-Path $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList @(
            "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=`"$work\uninstall.log`""
        ) -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "Uninstaller failed: $($process.ExitCode)" }
    }
}
foreach ($key in $owned) {
    if (Test-Path -LiteralPath $key) { throw "Registry key survived uninstall: $key" }
}
if (Test-Path -LiteralPath (Join-Path $install "rustzip.exe")) {
    throw "Executable survived uninstall"
}
$zipOpen = Get-Item -LiteralPath "$classes\.zip\OpenWithProgids" -ErrorAction SilentlyContinue
if ($null -ne $zipOpen -and $zipOpen.GetValueNames() -contains "RustZIP.Archive") {
    throw "ZIP Open With registration survived uninstall"
}
$zipAfterUninstall = if (Test-Path -LiteralPath $zipKey) { (Get-Item -LiteralPath $zipKey).GetValue("") } else { $null }
if ($zipAfterUninstall -ne $zipBefore) { throw "Uninstall changed the ZIP default handler" }
Write-Host "Windows installer smoke test passed; logs: $work"
