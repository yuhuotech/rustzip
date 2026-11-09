param(
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$BinaryDir = "$PWD\target\release",
    [string]$OutputDir = "$PWD\dist",
    [switch]$RarEnabled
)
$ErrorActionPreference = "Stop"
$payload = Join-Path $OutputDir "windows-payload"
if (Test-Path -LiteralPath $payload) { Remove-Item -LiteralPath $payload -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $payload "licenses") -Force | Out-Null
foreach ($name in @("rustzip.exe", "rz-updater.exe", "rz-pdf-renderer.exe", "rz-video-renderer.exe")) {
    Copy-Item -LiteralPath (Join-Path $BinaryDir $name) -Destination $payload
}
$licenses = @{
    "pbz2.txt"="pbz2.txt"; "pbz2-notice.txt"="pbz2-notice.txt"; "libchm.txt"="libchm.txt";
    "format-readers.txt"="format-readers.txt"; "pdf-renderer.txt"="pdf-renderer.txt";
    "audio-playback.txt"="audio-playback.txt"; "rodio-mit.txt"="rodio-mit.txt";
    "rodio-apache-2.0.txt"="rodio-apache-2.0.txt"; "cpal-apache-2.0.txt"="cpal-apache-2.0.txt";
    "symphonia-mpl-2.0.txt"="symphonia-mpl-2.0.txt"; "video-preview.txt"="video-preview.txt";
    "mp4-mit.txt"="mp4-mit.txt"; "rust-h264-mit.txt"="rust-h264-mit.txt";
    "rust-h264-apache-2.0.txt"="rust-h264-apache-2.0.txt"; "reflink-copy-mit.txt"="reflink-copy-mit.txt";
    "inno-chinese-translation.txt"="inno-chinese-translation.txt"
}
foreach ($entry in $licenses.GetEnumerator()) {
    Copy-Item -LiteralPath (Join-Path "$PSScriptRoot\..\licenses" $entry.Key) -Destination (Join-Path "$payload\licenses" $entry.Value)
}
$share = Join-Path $env:VCPKG_INSTALLATION_ROOT "installed\x64-windows-static-md\share"
foreach ($name in @("zlib", "bzip2", "liblzma", "zstd", "lz4", "openssl")) {
    Copy-Item -LiteralPath (Join-Path $share "$name\copyright") -Destination (Join-Path "$payload\licenses" "$name.txt")
}
if ($RarEnabled) { Copy-Item -LiteralPath "$PSScriptRoot\..\licenses\unrar.txt" -Destination (Join-Path $payload "unrar-license.txt") }
$layout = @{schema=1; version=$Version; platform="windows-x86_64-zip"} | ConvertTo-Json -Compress
[IO.File]::WriteAllText((Join-Path $payload "update-layout.json"), $layout, [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath (Join-Path $payload "update-layout.json") -Destination (Join-Path $BinaryDir "update-layout.json")
$archive = Join-Path $OutputDir "RustZIP-$Version-windows-x86_64-update.zip"
Compress-Archive -Path "$payload\*" -DestinationPath $archive -Force
Write-Host "Windows update ZIP created: $archive"
