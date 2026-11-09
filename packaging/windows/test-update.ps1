$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
$registration = "HKCU:\Software\Yuhuo\RustZIP"
if (Test-Path -LiteralPath $registration) { throw "Updater smoke tests require an account without a RustZIP installation." }
& cargo build --manifest-path "$root\Cargo.toml" -p rz-updater --features test-fixture
if ($LASTEXITCODE -ne 0) { throw "Fixture build failed" }
$bin = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR "debug" } else { Join-Path $root "target\debug" }
$work = Join-Path $env:TEMP ("RustZIP-update-ci-" + [guid]::NewGuid().ToString("N"))
$source = Join-Path $work "source"
New-Item -ItemType Directory -Path "$source\licenses" -Force | Out-Null
try {
    foreach ($name in @("rustzip.exe", "rz-pdf-renderer.exe", "rz-video-renderer.exe")) { Copy-Item -LiteralPath "$bin\rz-update-fixture.exe" -Destination (Join-Path $source $name) }
    Copy-Item -LiteralPath "$bin\rz-updater.exe" -Destination "$source\rz-updater.exe"
    [IO.File]::WriteAllText("$source\licenses\mit.txt", "test license")
    $version = ((& cargo metadata --manifest-path "$root\Cargo.toml" --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq "rz-updater").version
    if ($LASTEXITCODE -ne 0) { throw "Metadata failed" }
    [IO.File]::WriteAllText("$source\update-layout.json", (@{schema=1;version=$version;platform="windows-x86_64-zip"}|ConvertTo-Json -Compress))
    foreach ($mode in @("success", "fail", "interrupted")) {
        $install = Join-Path $work "$mode app 中文"
        Copy-Item -LiteralPath $source -Destination $install -Recurse
        [IO.File]::WriteAllText("$install\rustzip-install.json", (@{schema=1;kind="windows";root=$install;machine=$false}|ConvertTo-Json -Compress))
        [IO.File]::WriteAllText("$install\unins000.exe", "retain uninstaller")
        [IO.File]::WriteAllText("$install\custom.txt", "retain custom data")
        New-Item -Path $registration -Force | Out-Null
        New-ItemProperty -Path $registration -Name InstallDir -Value $install -PropertyType String -Force | Out-Null
        $download = Join-Path $work "RustZIP-update-$mode"
        New-Item -ItemType Directory -Path $download | Out-Null
        $result = Join-Path $work "result-$mode.txt"
        $env:RZ_UPDATE_FIXTURE_RESULT = $result
        $env:RZ_UPDATE_FIXTURE_MODE = $mode
        $env:RZ_UPDATE_FIXTURE_FAULT = if ($mode -eq "interrupted") { "published" } else { "" }
        & "$install\rustzip.exe" --fixture-update $source "$download\update.zip"
        if ($LASTEXITCODE -ne 0) { throw "Fixture handoff failed" }
        if ($mode -eq "interrupted") {
            for ($attempt=0; $attempt -lt 100; $attempt++) {
                $journals = @(Get-ChildItem -LiteralPath $work -Filter ".rustzip-*.json" -File | Where-Object { (Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json).phase -eq "Published" })
                if ($journals.Count -gt 0) { break }
                Start-Sleep -Milliseconds 100
            }
            if ($journals.Count -ne 1) { throw "Interrupted transaction was not persisted" }
            Start-Sleep -Milliseconds 300
            if (Test-Path -LiteralPath $result) { throw "Interrupted updater unexpectedly restarted the program" }
            Remove-Item Env:RZ_UPDATE_FIXTURE_FAULT -ErrorAction SilentlyContinue
            & "$install\rustzip.exe"
            if ($LASTEXITCODE -ne 0) { throw "Startup recovery handoff failed" }
        }
        for ($attempt=0; $attempt -lt 600 -and -not (Test-Path -LiteralPath $result); $attempt++) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path -LiteralPath $result)) { throw "Updater did not restart the fixture" }
        $expected = if ($mode -eq "success") { "updated" } else { "rolled-back" }
        if ([IO.File]::ReadAllText($result) -ne $expected) { throw "Wrong update result" }
        Start-Sleep -Milliseconds 500
        $layout = Get-Content -LiteralPath "$install\update-layout.json" -Raw | ConvertFrom-Json
        $expectedVersion = if ($mode -eq "success") { "0.2.0" } else { $version }
        if ($layout.version -ne $expectedVersion) { throw "Wrong installed version" }
        if ([IO.File]::ReadAllText("$install\unins000.exe") -ne "retain uninstaller") { throw "Uninstaller was changed" }
        if ([IO.File]::ReadAllText("$install\custom.txt") -ne "retain custom data") { throw "User files were changed" }
        Remove-Item -LiteralPath $registration -Recurse -Force
    }
} finally {
    Remove-Item Env:RZ_UPDATE_FIXTURE_RESULT -ErrorAction SilentlyContinue
    Remove-Item Env:RZ_UPDATE_FIXTURE_MODE -ErrorAction SilentlyContinue
    Remove-Item Env:RZ_UPDATE_FIXTURE_FAULT -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $registration -Recurse -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "Windows signed ZIP update, restart, rollback and uninstaller preservation passed."
