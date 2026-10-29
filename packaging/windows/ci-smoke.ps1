param(
    [switch]$RarEnabled,
    [string]$OutputDir
)

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
    "$classes\*\shell\RustZIP.Compress",
    "$classes\Directory\shell\RustZIP.Compress",
    "$classes\SystemFileAssociations\.zip\shell\RustZIP.Extract"
)
$rarContext = "$classes\SystemFileAssociations\.rar\shell\RustZIP.Extract"
if ($RarEnabled) { $owned += $rarContext }
foreach ($key in $owned) {
    if (Test-Path -LiteralPath $key) { throw "Smoke test requires a clean account: $key" }
}

$work = Join-Path $env:RUNNER_TEMP ("rustzip-installer-" + [guid]::NewGuid().ToString("N"))
$install = Join-Path $work "app"
$dist = if ([string]::IsNullOrWhiteSpace($OutputDir)) { Join-Path $work "dist" } else { $OutputDir }
New-Item -ItemType Directory -Path $dist | Out-Null
$exe = Join-Path $PWD "target\release\rustzip.exe"
$zipKey = "$classes\.zip"
$zipBefore = if (Test-Path -LiteralPath $zipKey) { (Get-Item -LiteralPath $zipKey).GetValue("") } else { $null }

$buildArgs = @("build", "--release", "-p", "rustzip-gui")
if ($RarEnabled) { $buildArgs += @("--features", "rar") }
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw "Windows GUI build failed: $LASTEXITCODE" }
$compilerArgs = @("/DAppVersion=0.1.0", "/DSourceExe=$exe", "/DOutDir=$dist")
if ($RarEnabled) { $compilerArgs += "/DRarEnabled" }
$compilerArgs += "packaging\windows\rustzip.iss"
& $iscc @compilerArgs
if ($LASTEXITCODE -ne 0) { throw "Windows installer compilation failed: $LASTEXITCODE" }
$installer = Join-Path $dist "RustZIP-0.1.0-windows-x86_64.exe"
if (-not (Test-Path $installer)) { throw "Windows installer was not created" }

try {
    $process = Start-Process -FilePath $installer -ArgumentList @(
        "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART",
        "/DIR=`"$install`"", "/TASKS=contextmenu,openwith", "/LOG=`"$work\install.log`""
    ) -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Installer failed: $($process.ExitCode)" }
    $installedFiles = @("rustzip.exe", "licenses\zlib.txt", "licenses\libchm.txt", "licenses\pbz2.txt", "licenses\pbz2-notice.txt", "licenses\inno-chinese-translation.txt")
    if ($RarEnabled) { $installedFiles += "unrar-license.txt" }
    foreach ($file in $installedFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $install $file))) {
            throw "Installed file missing: $file"
        }
    }
    foreach ($key in $owned) {
        if (-not (Test-Path -LiteralPath $key)) { throw "Expected registry key missing: $key" }
    }
    $application = Get-Item -LiteralPath "$classes\Applications\rustzip.exe"
    if ($application.GetValue("FriendlyAppName") -ne "RustZIP") {
        throw "Open With application name is missing"
    }
    $openCommand = Get-Item -LiteralPath "$classes\Applications\rustzip.exe\shell\open\command"
    if ($openCommand.GetValue("") -notlike "*$install*rustzip.exe*") {
        throw "Open With application command is incorrect"
    }
    $zipOpen = Get-Item -LiteralPath "$classes\.zip\OpenWithProgids"
    if ($zipOpen.GetValueNames() -notcontains "RustZIP.Archive") {
        throw "ZIP Open With registration is missing"
    }
    $rarOpen = Get-Item -LiteralPath "$classes\.rar\OpenWithProgids" -ErrorAction SilentlyContinue
    $rarRegistered = $null -ne $rarOpen -and $rarOpen.GetValueNames() -contains "RustZIP.Archive"
    if ($RarEnabled -and -not $rarRegistered) {
        throw "RAR Open With registration is missing from the RAR package"
    }
    if (-not $RarEnabled -and $rarRegistered) {
        throw "Default build must not register RAR"
    }
    if ($RarEnabled -and -not (Test-Path -LiteralPath $rarContext)) {
        throw "RAR Extract here context-menu registration is missing"
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
$application = Get-Item -LiteralPath "$classes\Applications\rustzip.exe" -ErrorAction SilentlyContinue
if ($application -and ($application.GetValueNames().Count -gt 0 -or $application.GetSubKeyNames().Count -gt 0)) {
    throw "Open With application registration survived uninstall"
}
if (Test-Path -LiteralPath (Join-Path $install "rustzip.exe")) {
    throw "Executable survived uninstall"
}
$zipOpen = Get-Item -LiteralPath "$classes\.zip\OpenWithProgids" -ErrorAction SilentlyContinue
if ($null -ne $zipOpen -and $zipOpen.GetValueNames() -contains "RustZIP.Archive") {
    throw "ZIP Open With registration survived uninstall"
}
$rarOpen = Get-Item -LiteralPath "$classes\.rar\OpenWithProgids" -ErrorAction SilentlyContinue
if ($null -ne $rarOpen -and $rarOpen.GetValueNames() -contains "RustZIP.Archive") {
    throw "RAR Open With registration survived uninstall"
}
$zipAfterUninstall = if (Test-Path -LiteralPath $zipKey) { (Get-Item -LiteralPath $zipKey).GetValue("") } else { $null }
if ($zipAfterUninstall -ne $zipBefore) { throw "Uninstall changed the ZIP default handler" }
Write-Host "Windows installer smoke test passed; logs: $work"
