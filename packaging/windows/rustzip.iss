; Inno Setup script for RustZIP. Build with:
;   iscc /DAppVersion=0.1.0 /DSourceExe=..\..\target\release\rustzip.exe /DOutDir=..\..\dist packaging\windows\rustzip.iss
;
; It installs per user (no administrator rights), registers "Open with RustZIP" for the
; archive types, and adds Compress / Extract here to the Explorer right-click menu.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\rustzip.exe"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif
#ifndef RarEnabled
  #define RarEnabled 0
#endif
#define VcpkgRoot GetEnv("VCPKG_INSTALLATION_ROOT")
#ifndef VcpkgShare
  #if VcpkgRoot == ""
    #error Set VCPKG_INSTALLATION_ROOT or pass /DVcpkgShare with the x64-windows-static-md share directory.
  #endif
  #define VcpkgShare VcpkgRoot + "\installed\x64-windows-static-md\share"
#endif

[Setup]
AppId={{6F2A5B1E-3C0D-4E0B-9B8A-5D1F7A9C2E41}
AppName=RustZIP
AppVersion={#AppVersion}
AppPublisher=RustZIP
AppPublisherURL=https://github.com/yuhuotech/rustzip
DefaultDirName={autopf}\RustZIP
DefaultGroupName=RustZIP
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutDir}
OutputBaseFilename=RustZIP-{#AppVersion}-windows-x86_64
SetupIconFile=..\icons\rustzip.ico
UninstallDisplayIcon={app}\rustzip.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
ChangesAssociations=yes

[Languages]
; MIT translation pinned from kira-96/Inno-Setup-Chinese-Simplified-Translation.
Name: "chinesesimplified"; MessagesFile: "ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "contextmenu"; Description: "在资源管理器右键菜单中添加“用 RustZIP 压缩”和“解压到此处”"; GroupDescription: "系统集成:"
Name: "openwith"; Description: "在“打开方式”中显示 RustZIP(不会改变默认程序)"; GroupDescription: "系统集成:"
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "快捷方式:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#VcpkgShare}\zlib\copyright"; DestDir: "{app}\licenses"; DestName: "zlib.txt"; Flags: ignoreversion
Source: "{#VcpkgShare}\bzip2\copyright"; DestDir: "{app}\licenses"; DestName: "bzip2.txt"; Flags: ignoreversion
Source: "{#VcpkgShare}\liblzma\copyright"; DestDir: "{app}\licenses"; DestName: "liblzma.txt"; Flags: ignoreversion
Source: "{#VcpkgShare}\zstd\copyright"; DestDir: "{app}\licenses"; DestName: "zstd.txt"; Flags: ignoreversion
Source: "{#VcpkgShare}\lz4\copyright"; DestDir: "{app}\licenses"; DestName: "lz4.txt"; Flags: ignoreversion
Source: "{#VcpkgShare}\openssl\copyright"; DestDir: "{app}\licenses"; DestName: "openssl.txt"; Flags: ignoreversion
Source: "..\licenses\inno-chinese-translation.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
#if RarEnabled == 1
Source: "..\licenses\unrar.txt"; DestDir: "{app}"; DestName: "unrar-license.txt"; Flags: ignoreversion
#endif

[Icons]
Name: "{group}\RustZIP"; Filename: "{app}\rustzip.exe"
Name: "{autodesktop}\RustZIP"; Filename: "{app}\rustzip.exe"; Tasks: desktopicon

[Registry]
; The program itself, so Windows can list it under "Open with".
Root: HKCU; Subkey: "Software\Classes\Applications\rustzip.exe\shell\open\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" ""%1"""; Tasks: openwith; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\rustzip.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "RustZIP"; Tasks: openwith; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\.7z\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.zip\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
#if RarEnabled == 1
Root: HKCU; Subkey: "Software\Classes\.rar\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
#endif
Root: HKCU; Subkey: "Software\Classes\.tar\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.gz\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tgz\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.xz\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.txz\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.zst\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tzst\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.bz2\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.tbz2\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.lz4\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.iso\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.cab\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.cpio\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.ar\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.deb\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.rpm\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.lha\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.lzh\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.xar\OpenWithProgids"; ValueType: string; ValueName: "RustZIP.Archive"; ValueData: ""; Tasks: openwith; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\RustZIP.Archive"; ValueType: string; ValueData: "压缩文件"; Tasks: openwith; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\RustZIP.Archive\DefaultIcon"; ValueType: string; ValueData: "{app}\rustzip.exe,0"; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\RustZIP.Archive\shell\open\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" ""%1"""; Tasks: openwith

; Right-click on any file or folder: compress it.
Root: HKCU; Subkey: "Software\Classes\*\shell\RustZIP.Compress"; ValueType: string; ValueData: "用 RustZIP 压缩"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\*\shell\RustZIP.Compress"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\rustzip.exe,0"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\*\shell\RustZIP.Compress\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --compress ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\RustZIP.Compress"; ValueType: string; ValueData: "用 RustZIP 压缩"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\RustZIP.Compress"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\rustzip.exe,0"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\RustZIP.Compress\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --compress ""%1"""; Tasks: contextmenu

; Right-click on an archive: extract next to it.
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
#if RarEnabled == 1
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
#endif
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lz4\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lz4\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.iso\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.iso\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.cab\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.cab\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.cpio\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.cpio\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.ar\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.ar\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.deb\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.deb\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.rpm\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.rpm\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lha\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lha\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lzh\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.lzh\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.xar\shell\RustZIP.Extract"; ValueType: string; ValueData: "用 RustZIP 解压到此处"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\SystemFileAssociations\.xar\shell\RustZIP.Extract\command"; ValueType: string; ValueData: """{app}\rustzip.exe"" --extract-here ""%1"""; Tasks: contextmenu

[Run]
Filename: "{app}\rustzip.exe"; Description: "启动 RustZIP"; Flags: nowait postinstall skipifsilent
