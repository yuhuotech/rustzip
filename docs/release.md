# 发布、更新与统计

## 一次性准备

1. **更新签名密钥**。公钥已写进 `crates/rz-services/src/update_key.in`,私钥由 `rz-release keygen` 生成,在维护者本机 `~/.rustzip-release/update-signing.key`(权限 600)。
   - 把这个文件的内容(64 个十六进制字符)存成 GitHub 仓库的 **Secret `RZ_UPDATE_KEY`**。
   - **私钥不进仓库,也不要发给任何人。** 丢了只能换密钥:旧版本不认新公钥,用户要手动重装一次。
   - 备份到密码管理器。
2. **统计服务器地址**(可选):先按 [`web/README.md`](../web/README.md) 部署官网和 Neon 数据库,然后把 `https://rustzip.vercel.app/api/v1/telemetry` 设为 GitHub 仓库 **Variable `RZ_STATS_URL`**。以后切换到 `rustzip.com` 时更新该变量即可。没设就不会上报任何东西。
3. **macOS 签名**(可选,强烈建议):没有 Apple 开发者账号时 dmg 只做临时签名,用户首次打开要在"系统设置 → 隐私与安全"里点"仍要打开"。有账号则设置 Secrets:`MACOS_CERT_P12`(base64)、`MACOS_CERT_PASSWORD`、`MACOS_SIGN_IDENTITY`,公证再加 `APPLE_ID`、`APPLE_TEAM_ID`、`APPLE_APP_PASSWORD`。
4. **Windows 签名**:目前不签。SmartScreen 会提示"未知发布者",买代码签名证书后再接入。

## 发一个版本

> 当前 GitHub Actions 自动触发暂时关闭：推送 `main`、PR 和推送版本 tag 都不会自动构建。CI 可从 Actions 页面手动运行；Release 工作流仅保留手动触发，并且只在 `v*` tag 上执行。

1. 改根 `Cargo.toml` 里 `[workspace.package] version`,提交。
2. `git tag v0.2.0 && git push origin v0.2.0`。
3. `.github/workflows/release.yml` 在三个系统上打包(macOS arm64 和 x86_64 各一个 dmg,Windows 一个安装程序,Linux 一个 tar.gz),用 `RZ_UPDATE_KEY` 给每个安装包签名,汇总成 `update.json`,创建 GitHub Release 并上传。
4. 已安装的用户下次启动时,从 `https://github.com/yuhuotech/rustzip/releases/latest/download/update.json` 读到新版本,状态栏出现"新版本 x.y.z · 更新"。

## 更新怎么保证安全

- `update.json` 每个条目带 SHA-256 和 Ed25519 签名,签名内容是"版本 + 平台 + 文件哈希"。
- 客户端只信内置公钥,不信 GitHub、TLS 或重定向:大小、哈希、签名任何一项不对,下载直接丢弃。
- 只接受 https;不会降级到旧版本;稳定版用户不会被推送预发布版。
- 下载后只是"保存到临时目录并交给系统打开"(macOS 打开 dmg,Windows 运行安装程序,Linux 打开所在文件夹),**不会静默替换正在运行的程序**。

## 匿名统计

上报内容固定 7 个字段,没有别的(代码里 `rz_app::stats::Report`,新增字段属于隐私变更,必须同步改首次启动提示):

```json
{"event":"activate|heartbeat","install_id":"随机 UUID","app_version":"0.1.0",
 "os":"macos","os_version":"15.1","arch":"aarch64","language":"zh-CN"}
```

- 首次启动弹说明,用户点"好的"之后才会上报;点"不参与"则永不上报,也不生成安装编号。
- `activate` 只发一次;之后每天最多一次 `heartbeat`。失败直接丢弃,下次启动再试,不阻塞启动。
- 设置里可随时关闭。

服务端由仓库 `web/` 提供:`POST /api/v1/telemetry` 只接受上述 7 个字段,按安装 ID 和日期幂等去重。原始 UUID 在写入数据库前用 HMAC-SHA-256 转换;不保存文件信息或请求 IP。健康检查为 `GET /api/health`。数据库查询和本地验证方法见 [`web/README.md`](../web/README.md)。

Vercel build 会在部署期间自动应用 Neon Postgres migrations;检查部署日志确认迁移成功,并确认 `/api/health` 返回 200。域名配置完成后,再将完整统计地址写入 GitHub Repository Settings → Secrets and variables → Actions → Variables → `RZ_STATS_URL`。应用更新仍使用 GitHub Release 内的签名 `update.json`,不依赖 Vercel。

## 本地打包(不发布)

| 系统 | 命令 | 产物 |
|---|---|---|
| macOS | `packaging/macos/build-app.sh dist` | `RustZIP.app` 和 `RustZIP-<版本>-macos-<架构>.dmg` |
| Windows | `iscc /DAppVersion=0.1.0 packaging\windows\rustzip.iss`(需先安装 x64-windows vcpkg 依赖并运行 `cargo build --release -p rustzip-gui`) | `RustZIP-<版本>-windows-x86_64.exe` |
| Linux | `packaging/linux/build-tarball.sh dist` | `RustZIP-<版本>-linux-<架构>.tar.gz`,解开后 `./install.sh` |

图标由 `cargo run --release -p rustzip-gui --example make_icons -- packaging/icons` 生成(`.icns`、`.ico`、各尺寸 PNG)。

默认包不启用 RAR，也不注册 RAR 专属关联。macOS / Linux 可设置 `RZ_WITH_RAR=1` 打包测试版；Windows 需用 `cargo build --release -p rustzip-gui --features rar` 构建，并给 Inno Setup 增加 `/DRarEnabled=1`。启用时安装包附带 unRAR 许可证。RAR 临时磁盘输出尚无硬上限，不能将此测试包视为已完成安全验收的正式发行版，见 D35 / D36。

Windows 安装器需要 `VCPKG_INSTALLATION_ROOT` 指向包含 `installed\x64-windows\bin` 和 `share` 的 vcpkg；也可以分别用 `/DVcpkgBin=...`、`/DVcpkgShare=...` 指定。安装器只打包列出的归档后端运行时 DLL 和相应许可文件；这些文件名及依赖加载尚待 Windows 原生打包和卸载验收。

Windows MSVC 的 `libarchive2-sys` 需要 vcpkg 导入库。安装依赖后，按 CI 的 `native-archive-deps` action 将 `z.lib` 复制为 `zlib.lib`，并将 `lzma.lib` 复制为 `rustzip_lzma_import.lib`；后者避免被 Rust 编码器的同名静态库遮蔽。构建和运行测试时还需将 vcpkg 的 `installed\x64-windows\bin` 加入 `PATH`。这些步骤仍需在 Windows runner 完整复测。

## 系统集成现状

| 能力 | macOS | Windows | Linux |
|---|---|---|---|
| 双击/"打开方式"打开压缩包 | 已做(Info.plist 文档类型 + 系统打开事件) | 已做(安装程序注册,不抢默认程序) | 已做(`.desktop` 的 MimeType) |
| 右键"用 RustZIP 压缩"/"解压到此处" | **未做**(需要 Finder 扩展或 Services,另做) | 已做(安装程序写注册表) | Dolphin 服务菜单、Nemo 动作已做;Nautilus 50.2.2 的 Scripts 子菜单、压缩入口和逐字节解压已在 Ubuntu 26.04 / Xvfb 验收 |
| 预览(Quick Look 等) | 未做 | 未做 | 未做 |

macOS 的“打开方式”声明包含已接入的常见只读格式及 LZ4，RAR 仅在显式启用时声明；声明本身不改变系统默认程序，实际 Finder 展示仍需在安装后的系统上验收。

命令行入口:`rustzip --extract-here <压缩包>...` 解压到压缩包所在目录;`rustzip --compress <文件或文件夹>...` 打开新建压缩窗口。

**验证状态**：macOS arm64 `.app` / DMG 的打包、签名校验和启动通过，尚未公证；Linux x86_64 tarball 已在 Ubuntu 26.04 构建，并通过隔离安装 / 卸载和 Nautilus Scripts 菜单验收。Windows 安装程序和完整 release 工作流尚未运行。当前 GitHub Actions 只允许手动触发 CI；Release 需在 `v*` tag 上手动运行。仓库截至 2026-10-27 尚无公开 GitHub Release。

2026-10-28 的手动 CI、macOS 钥匙串 / Launch Services 检查及 Windows 链接失败与后续修复状态见 [第二阶段验收](phase-2-platform-acceptance.md)；修复后的工作区尚未在 Windows 或远端 CI 验证。
