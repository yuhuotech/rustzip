# 发布、更新与统计

## 一次性准备

1. **更新签名密钥**。公钥已写进 `crates/rz-services/src/update_key.in`,私钥由 `rz-release keygen` 生成,在维护者本机 `~/.rustzip-release/update-signing.key`(权限 600)。
   - 把这个文件的内容(64 个十六进制字符)存成 GitHub 仓库的 **Secret `RZ_UPDATE_KEY`**。
   - **私钥不进仓库,也不要发给任何人。** 丢了只能换密钥:旧版本不认新公钥,用户要手动重装一次。
   - 备份到密码管理器。
2. **统计服务器地址**(可选):先按 [`web/README.md`](../web/README.md) 部署官网和 Neon 数据库,然后把 `https://rustzip.vercel.app/api/v1/telemetry` 设为 GitHub 仓库 **Variable `RZ_STATS_URL`**。2026-10-30 已确认官网、`/formats`、`/privacy` 与 `/api/health` 均返回 HTTP 200，数据库健康；仓库变量已设置。以后切换到 `rustzip.com` 时更新该变量即可。变量为空时客户端不发送统计。
3. **macOS 签名**(可选,强烈建议):没有 Apple 开发者账号时 dmg 只做临时签名,用户首次打开要在"系统设置 → 隐私与安全"里点"仍要打开"。有账号则设置 Secrets:`MACOS_CERT_P12`(base64)、`MACOS_CERT_PASSWORD`、`MACOS_SIGN_IDENTITY`,公证再加 `APPLE_ID`、`APPLE_TEAM_ID`、`APPLE_APP_PASSWORD`。
4. **Windows 签名**:目前不签。SmartScreen 会提示"未知发布者",买代码签名证书后再接入。

## 发一个版本

> 主 CI 和 Release workflow 由 Actions 页面手动触发。Release dry-run 可从任意分支运行；正式发布只接受 `v*` tag。Fuzz workflow 当前配置为每日 02:07 UTC 运行 ZIP recovery、CHM、bzip2、RAR、NSIS、PDF、音频和视频八个 target，不构建或发布安装包；可手动启用 `verify_crash_artifact` 对受控 sentinel crash 的 artifact 上传做端到端检查。run [41046064115](https://github.com/yuhuotech/rustzip/actions/runs/41046064115) 已通过 artifact smoke；新 schedule 仍待远端触发验收。

1. 改根 `Cargo.toml` 里 `[workspace.package] version`,提交。
2. 先在 `main` 上手动触发 Release workflow，保留默认 `dry_run=true`；它会生成一次性测试密钥、构建四个平台产物、签名并校验平台集合和实际文件，保存名为 `release-dry-run-bundle` 的 14 天 artifact。测试签名不匹配应用公钥，dry-run 包不能用于更新安装。
3. 检查 dry-run artifact 后创建并推送对应的 `v*` tag，再从该 tag 手动触发 Release workflow 并将 `dry_run` 设为 `false`。workflow 会再次检查 tag 与 `Cargo.toml` 版本一致，只有四个平台资产全部存在且客户端公钥验签通过才会发布。
4. 发布后 `.github/workflows/release.yml` 创建 GitHub Release 并上传四个平台安装包和 `update.json`。
5. 已安装的用户下次启动时,从 `https://github.com/yuhuotech/rustzip/releases/latest/download/update.json` 读到新版本,状态栏出现"新版本 x.y.z · 更新"。

Dry-run 默认不创建 GitHub Release，也不提交 Apple notarization 请求。其一次性私钥只在 1 天有效的单独 workflow artifact 中供 matrix jobs 共享，最终 dry-run bundle 不包含该密钥。正式发布使用已经安全保管的 `RZ_UPDATE_KEY`，并以应用内置公钥验签；生产私钥不会写入仓库或任何 workflow artifact。

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

Vercel build 会在部署期间自动应用 Neon Postgres migrations;检查部署日志确认迁移成功,并确认 `/api/health` 返回 200。2026-10-30 线上健康检查返回 `{"status":"ok","database":"ok"}`，仓库 `RZ_STATS_URL` 已设置为完整接口地址。`rz-services/build.rs` 声明该变量为 Cargo build input，变量改动会重建并更新桌面包里的默认统计地址；本机在同一 target 缓存目录中验证了“未设置 → 设置 URL”会重编译并嵌入正确地址。应用更新仍使用 GitHub Release 内的签名 `update.json`,不依赖 Vercel。

## 本地打包(不发布)

| 系统 | 命令 | 产物 |
|---|---|---|
| macOS | `packaging/macos/build-app.sh dist` | `RustZIP.app` 和 `RustZIP-<版本>-macos-<架构>.dmg` |
| Windows | `iscc /DAppVersion=0.1.0 packaging\windows\rustzip.iss`(需先安装 x64-windows vcpkg 依赖并运行 `cargo build --release -p rustzip-gui -p rz-pdf-renderer`) | `RustZIP-<版本>-windows-x86_64.exe` |
| Linux | `packaging/linux/build-tarball.sh dist` | `RustZIP-<版本>-linux-<架构>.tar.gz`,解开后 `./install.sh` |

图标由 `cargo run --release -p rustzip-gui --example make_icons -- packaging/icons` 生成(`.icns`、`.ico`、各尺寸 PNG)。

默认包不启用 RAR，也不注册 RAR 专属关联。macOS / Linux 可设置 `RZ_WITH_RAR=1` 打包测试版；Windows 需用 `cargo build --release -p rustzip-gui -p rz-pdf-renderer --features rustzip-gui/rar` 构建，并给 Inno Setup 增加 `/DRarEnabled`。启用时安装包附带 unRAR 许可证。RAR 解码现由 `rz-ffi` 逐块执行并受选中输出总量、条目声明大小和取消控制；四平台 opt-in 包、许可证和 Windows 安装 / 卸载关联已在 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 验证，正式签名 Release 仍待验收，见 D36 / D38。

在手动触发 Release dry-run 时，可将 `validate_rar_packages=true`，额外构建 macOS arm64 / x86_64、Linux x86_64 和 Windows x86_64 的 RAR 测试安装包。此验证会检查 unRAR 许可证、macOS 类型声明、Linux MIME 声明以及 Windows 安装 / 卸载后的 RAR 关联；生成的 `rar-package-validation-bundle` 单独保存 14 天，不进入更新清单或公开 Release。该选项默认关闭。

Windows 使用 `x64-windows-static-md` vcpkg triplet 静态链接原生归档库，避免 libarchive 与 Rust LZMA 后端的 DLL 导入符号冲突。安装器从 `VCPKG_INSTALLATION_ROOT` 收集原生依赖许可证；原生安装与卸载仍待 Windows runner 验收。

Windows MSVC 构建通过 CI 的 `native-archive-deps` action 安装 `x64-windows-static-md` vcpkg 库，将 zlib 的 `zs.lib` 适配为 `libarchive2-sys` 使用的 `zlib.lib`，并清理 `libarchive2-sys` 旧构建结果以按新 triplet 重新检测压缩库。原生链接和测试仍需 Windows runner 实测。

## 2026-10-31 最新验证

- 更新提交 `62d5ccd` 的 Release dry-run [41157889524](https://github.com/yuhuotech/rustzip/actions/runs/41157889524) 正在对包含 H.264 8×8 bounds patch 的最新代码做四平台和 RAR 包验收；publish 仍由 dry-run 条件跳过。
- Release dry-run [41154738556](https://github.com/yuhuotech/rustzip/actions/runs/41154738556) 对 `0a5c460` 四个平台常规包、四个 RAR 验证包、Windows 安装 smoke、manifest 汇总验签及 artifact 上传全部成功；公开 `publish` 按 dry-run 条件跳过。
- CI [41154082845](https://github.com/yuhuotech/rustzip/actions/runs/41154082845) 对 `0a5c460` 的七个 job 全部通过。随后该提交的 video Fuzz [41154063015](https://github.com/yuhuotech/rustzip/actions/runs/41154063015) 发现 `rust_h264` 8×8 intra-block 越界；vendor bounds patch 已覆盖该路径并有 helper / renderer 回归，修补版 CI、Fuzz 与 Release dry-run 待重新触发。

## 系统集成现状

| 能力 | macOS | Windows | Linux |
|---|---|---|---|
| 双击/"打开方式"打开压缩包 | 已做(Info.plist 文档类型 + 系统打开事件) | 已做(安装程序注册,不抢默认程序) | 已做(`.desktop` 的 MimeType) |
| 右键"用 RustZIP 压缩"/"解压到此处" | Finder Sync 扩展已加入 `.app`，macOS 27.0.1 ad-hoc 测试包中两项均实际点击通过；用户需在系统设置启用扩展，Developer ID / 公证包仍待验收 | 已做(安装程序写注册表，原生菜单点击仍待验收) | 安装器写入 Dolphin 的 KIO `servicemenus` 与旧版 `kservices5/ServiceMenus`，并安装 Nemo 动作；Nautilus 50.2.2 的 Scripts 子菜单、压缩入口和逐字节解压已在 Ubuntu 26.04 / Xvfb 验收。Dolphin / Nemo 实机点击仍待验收 |
| 预览(Quick Look 等) | ZIP 目录视图扩展已打包并签名；Swift smoke 通过。macOS 27.0.1 临时 `.app` 经 Launch Services / PlugInKit 注册后 provider 可见并由 `qlmanage` 启动；Release dry-run [41043955928](https://github.com/yuhuotech/rustzip/actions/runs/41043955928) 的 macOS arm64 / x86_64 注册 smoke 通过并撤销临时状态。Finder 空格键可见页面与公证包仍待验收 | 未做 | 未做 |

macOS 的“打开方式”声明包含已接入的常见只读格式及 LZ4，RAR 仅在显式启用时声明；声明本身不改变系统默认程序，实际 Finder 展示仍需在安装后的系统上验收。

命令行入口:`rustzip --extract-here <压缩包>...` 解压到压缩包所在目录;`rustzip --compress <文件或文件夹>...` 打开新建压缩窗口。

**验证状态**：macOS arm64 `.app` / DMG 的打包、签名校验和启动通过，尚未公证；Finder Sync 扩展在 macOS 27.0.1 的 ad-hoc 测试包中完成右键压缩 / 解压点击验收。Linux x86_64 tarball 已在 Ubuntu 26.04 构建，并通过隔离安装 / 卸载和 Nautilus Scripts 菜单验收。Windows runner 已编译 GUI / Inno 安装器并完成自动安装 / 卸载冒烟；原生 GUI 人工验收和正式 Release 流程仍未完成。当前 GitHub Actions 只允许手动触发 CI；Release 需在 `v*` tag 上手动运行。仓库截至 2026-10-31 尚无公开 GitHub Release。

2026-10-31 仓库 Secrets 核对：当前未配置 Actions Secrets；`RZ_STATS_URL` Variable 已配置。正式签名 Release 需要维护者按本文流程添加 `RZ_UPDATE_KEY`，以及选择性配置 macOS Developer ID / notarization secrets；测试用 disposable key 只用于 dry-run。

- 2026-10-29 Release dry-run [40950855468](https://github.com/yuhuotech/rustzip/actions/runs/40950855468)：Linux、macOS arm64、macOS x86_64 与 Windows x86_64 产物均构建并签名成功；汇总验签 job 在构建 `rz-release` 时因 runner 未安装 `libacl` 开发库而失败，因此未生成验证 bundle，正式发布步骤跳过。已在汇总 job 加入 `native-archive-deps`；修复后的 dry-run 待重新运行。
- 2026-10-30 修复后的 Release dry-run [40958424663](https://github.com/yuhuotech/rustzip/actions/runs/40958424663) 全部通过：四个平台包均构建并签名，汇总验签成功，上传了 48,267,020 字节的 `release-dry-run-bundle` artifact；下载检查确认 `update.json` 含四个平台及对应包名。公开 Release 的 `publish` job 按 dry-run 条件跳过。该 bundle 可供 14 天检查，不是可安装更新的正式签名包。
- 2026-10-30 Release dry-run [40969974227](https://github.com/yuhuotech/rustzip/actions/runs/40969974227) 对 `b262828`：macOS arm64 / Linux x86_64 常规包构建成功；macOS arm64、x86_64 和 Linux 的 RAR 测试包均完成验证。Windows 常规包构建成功；RAR 测试包的 GUI 和 Inno Setup 均构建成功，但安装冒烟以退出码 1 失败。根因为脚本指定外部 `dist` 时只创建输出目录，却把 Inno `/LOG` 指向尚不存在的 `$work/install.log`；修复后见下一次 dry-run。
- 同一 Release run 的 macOS x86_64 常规打包在 plist 校验后退出，旧脚本未打印失败命令，汇总 job 因缺少该平台资产而失败。本机 arm64 使用旧版和当前 Finder Sync 代码交叉构建 x86_64 包均通过；修复后的 runner 验收见后续记录。
- 修复版 Release dry-run [40982574540](https://github.com/yuhuotech/rustzip/actions/runs/40982574540) 全部通过：四个平台常规包构建、签名和汇总验签成功；macOS arm64 / x86_64、Linux x86_64、Windows x86_64 的 RAR 包均构建并检查通过，Windows RAR 安装 / 卸载与关联冒烟通过。`release-dry-run-bundle` 和 `rar-package-validation-bundle` 成功上传；`publish` 按 dry-run 规则跳过，未创建公开 Release。
- 最终 Quick Look 安全边界提交 `0d58852` 的 Release dry-run [40994239520](https://github.com/yuhuotech/rustzip/actions/runs/40994239520) 全部通过：双架构 macOS、Windows、Linux 常规包及四目标 RAR 验证包构建成功，RAR 安装 / 卸载、manifest 验签和 bundles 上传通过；公开发布 job 按 dry-run 规则跳过。
- 当前 HEAD `67b4c2e` 的 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 全部通过：四个平台常规包构建 / 签名、四目标 RAR 包构建检查、Windows RAR 安装 / 关联 / 卸载冒烟、manifest 汇总验签及两个 bundles 上传成功；公开 `publish` job 按 dry-run 条件跳过，未创建正式 Release。
- 提交 `fabff8b` 的 Release dry-run [41026007832](https://github.com/yuhuotech/rustzip/actions/runs/41026007832) 全部通过：四个平台常规包构建签名、四平台 RAR 包验证、Windows RAR 安装 / 关联 / 卸载、manifest 汇总验签及两个 bundle 上传成功；`publish` 按 dry-run 条件跳过，未创建公开 Release。
- 提交 `fabff8b` 的三平台 CI [41026008129](https://github.com/yuhuotech/rustzip/actions/runs/41026008129) 七个 job 全部通过；五目标 Fuzz [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 均成功并保存 corpus cache。
- 提交 `51fffdb` 的 Release dry-run [41064921267](https://github.com/yuhuotech/rustzip/actions/runs/41064921267) 全部通过：四平台 Release 包及 RAR 验证包、各平台构建期 `RZ_STATS_URL` 子进程检查、manifest 汇总验签均成功；`publish` 按 dry-run 规则跳过。Fuzz run [41064921389](https://github.com/yuhuotech/rustzip/actions/runs/41064921389) 的五个 target 和受控 crash artifact 上传 smoke 全部通过。
- 提交 `6fd8e65` 的 Release dry-run [41079816983](https://github.com/yuhuotech/rustzip/actions/runs/41079816983) 四平台 Release 包 / RAR 验证、Windows RAR 安装冒烟、manifest 汇总验签均成功；`publish` 按 dry-run 规则跳过。Fuzz run [41079856580](https://github.com/yuhuotech/rustzip/actions/runs/41079856580) 的五个 target 和受控 crash artifact smoke 全部通过。定时事件在 13:37 UTC、workflow 重注册后的 16:53 UTC 和 17:35 UTC 均未观测到 run；cron 已恢复为 13:37 UTC，workflow 仍 active。
- 提交 `9deb72c` 的 Release dry-run [41090713832](https://github.com/yuhuotech/rustzip/actions/runs/41090713832) 四平台常规 / RAR 包、构建期遥测地址、macOS Quick Look 注册、Windows RAR 安装 / 卸载、manifest 汇总验签和两个 bundles 均通过；`publish` 按 dry-run 规则跳过，未创建公开 Release。
- 2026-10-31：提交 `ec871a0` 的 Release dry-run [41143963144](https://github.com/yuhuotech/rustzip/actions/runs/41143963144) 四个平台常规包 / RAR 验证包、Windows 安装冒烟、manifest 验签及 bundles 全部通过；publish 按 dry-run 条件跳过。提交 `ec871a0` 的 CI [41143963257](https://github.com/yuhuotech/rustzip/actions/runs/41143963257) 发现 Linux install smoke 测试夹具未带新增 reflink-copy MIT 文件；测试夹具已修正。Fuzz [41143902645](https://github.com/yuhuotech/rustzip/actions/runs/41143902645) 发现新的 WAV decoder panic，后续 run [41149879866](https://github.com/yuhuotech/rustzip/actions/runs/41149879866) 又发现 H.264 intra-block 越界；两项均已本地修复并加回归数据，最新代码的远端结果待重新验收。
- Release dry-run [41151267352](https://github.com/yuhuotech/rustzip/actions/runs/41151267352) 对 `bd59049` 的四平台常规包、RAR 验证包、Windows 安装冒烟、manifest 汇总验签和 bundles 上传全部通过；publish 按 dry-run 条件跳过。该提交早于 `rust_h264` vendor bounds patch；`0a5c460` 的新 Release dry-run [41154738556](https://github.com/yuhuotech/rustzip/actions/runs/41154738556) 正在运行。
- 本机 Finder Sync ad-hoc 验收包不等同正式签名 Release；签名密钥和公证配置仍未用于公开发布。
- 2026-10-29 最新 Fuzz 手动 run [40961694769](https://github.com/yuhuotech/rustzip/actions/runs/40961694769) 在提交 `70addea` 上四个 target 均通过并保存 corpus cache：ZIP recovery 743,952 次、CHM 34,172 次、bzip2 8,454 次、RAR 187,945 次，无 crash。每日 schedule 在 03:23 UTC；该次 run 在下次触发时间前结束。

2026-10-28 的手动 CI、macOS 钥匙串 / Launch Services 检查与 Windows 修复见[第二阶段验收](phase-2-platform-acceptance.md)。当前提交 `fabff8b` 的三平台 CI [41026008129](https://github.com/yuhuotech/rustzip/actions/runs/41026008129)、Release dry-run [41026007832](https://github.com/yuhuotech/rustzip/actions/runs/41026007832) 与五目标 Fuzz [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 均成功。正式签名 Release 和部分原生系统交互仍待完成。
