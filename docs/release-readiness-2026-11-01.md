# 正式版必需项开发与统一验收

用户确认：本批只完成正式版必需项；挂载、云读取、WASM 插件、SFX 保留远期计划。以下记录分开列出代码完成与实际验收，未执行的项目不标为通过。

2026-11-03 补充：下文为 11 月 1 日验收快照；后续已实现 WIM LZX / LZMS、NSIS bzip2、未加密单卷 APFS 和常见 APFS 透明压缩。三平台集成与正式签名仍需单独验收，具体支持边界见[第三阶段增量记录](phase-3-format-assessment.md)和[开发计划](next-development-plan.md)。

## 开发内容

- 视频：复现每日 Fuzz 41193338692 的 CABAC MVD 溢出，前缀有界解码、非法运动向量返回错误，原始样本进入 renderer、helper 和 Fuzz 回归。
- WIM：原生 stored / 32 KiB XPRESS-Huffman 资源读取，资源 SHA-1、目录循环 / 路径 / 范围检查、输出预算；XPRESS 局部 vendor 补丁补齐 buffered bits 与回溯输出硬上限。LZX / LZMS、跨卷、reparse / ADS 保持明确不支持。
- DMG：XML UDIF 零块 / raw / zlib，GPT / APM 和 HFS+ / HFSX catalog / inline extents，最多缓存一个 8 MiB 块；不挂载镜像。APFS、其他 codec、加密、extent overflow 和 hardlink 不支持；符号链接沿用引擎跳过策略。浏览核验 UDIF 主校验和及 GPT CRC；完整性测试还会按块检查存在的分区 / data fork CRC-32，支持取消、进度与大小预算。未提供校验值的区域只能检查可读性。
- NSIS：现代 Copy / Deflate / 标准 LZMA，支持非固实和最多 512 MiB 的有界固实流暂存；先验证 NSIS CRC，不执行安装器。自定义 bzip2 和旧布局不支持。
- 集成：GUI / CLI 共用 reader；WIM / DMG 关联、Finder 菜单、Linux MIME、常用位置识别；NSIS `.exe` 不自动关联。新依赖许可证随三平台安装包携带。
- 发布：校验真实更新私钥是否匹配客户端公钥，dry-run 继续使用一次性密钥。修复 macOS / Linux 打包脚本忽略 `CARGO_TARGET_DIR`、可能打入旧缓存二进制的问题。

## 统一验收清单

| 项目 | 状态 |
|---|---|
| 项目、fuzz、两个 vendor 的格式检查 | 通过；fuzz 全目标 check / all-features Clippy 通过 |
| Rust 1.96 工作区默认 / all-features Clippy 和测试 | 最终串行回归默认测试 510 通过、all-features 527 通过，均有 3 项环境测试默认忽略；其中两项非 root 权限测试已单独通过，真实系统钥匙串本批未重测；默认及 all-features Clippy 通过 |
| 新格式 CLI 列表 / 测试 / 提取 / 转换与内容比对 | 七个独立 WIM / DMG / NSIS fixture 通过；含 Unicode 和空文件；macOS ad-hoc GUI 实际显示 XPRESS WIM 文件列表 |
| release 编译、macOS app / DMG 与安装器 smoke | 本机全工作区 release 编译和 macOS arm64 ad-hoc app / DMG 构建通过，签名结构校验通过；Linux 安装 / 卸载脚本用隔离目录与 GNU install 冒烟通过；其他平台正式安装仍待实机 |
| 解压 / 压缩基准与 ZIP 编辑基准 | [本机基准与修改前对照](benchmarks/2026-11-01-format-reader-validation.md)已执行；样本及并行干扰不足以证明无回退，新格式大样本尚待补充 |
| 视频、NSIS、WIM、DMG ASan Fuzz | 视频 139,795 次、WIM 211,308 次、DMG 227,373 次、NSIS 78,759 次，均未发现 crash；NSIS 同时执行原始输入与重算 CRC 的输入以覆盖解码路径。视频和 WIM 各有一个 slow-unit，分别单次复放 2 ms / 1 ms，原长耗时可能受中断及休眠影响，不能作为超时通过的充分证据 |
| 依赖安全 / 许可 | `cargo deny check licenses advisories` 通过；保留 `deny.toml` 中原有的明确例外。新引入 quick-xml 0.39 曾触发 RUSTSEC-2026-0194 / 0195，已升级到 0.41 并重测 |
| 更新签名与篡改检测 | 本机对实际 DMG 使用一次性测试密钥签名并验签，修改文件后拒绝；不代表正式密钥及发布链路已验收 |
| 三平台 CI、Release dry-run 和远端 Fuzz | 新改动尚未提交或推送，远端工作流无法验证当前工作区；已有远端通过记录属于旧提交。`rz-core` / `rz-video-renderer` 已交叉检查 Windows x86_64 MSVC 与 Linux x86_64 GNU；这不是完整工作区构建，也不等于原生运行 |

本机默认 Cargo / Rustc 1.99 与仓库 Clippy 1.96 不一致，不能混用原 `target` 缓存。统一验证显式使用 Rust 1.96 的 Cargo、Rustc / Rustdoc，并使用独立的 `target/verify-1.96`。

macOS app 为临时 ad-hoc 签名；重新签名会改变包内 Mach-O 字节，不能直接用原始二进制与包内文件的 SHA-256 是否相同判断是否打入正确构建。打包脚本使用自定义 `CARGO_TARGET_DIR` 的路径已由测试和实际构建覆盖。

曾有一次并行执行时 PDF helper 的 watchdog 用例超过测试设定的 12 秒而失败；单独重跑在约 10.9 秒通过。产品 helper 的 10 秒限制没有改变，测试的等待余量调整到与视频 helper 一致的 16 秒，并重新运行完整回归。单次结果不能确认首次失败的唯一原因。

## 外部验收边界

- 正式发布需要维护者的更新签名 Secret；Developer ID / 公证需要 Apple 凭据。不会生成替代密钥冒充现有签名，也不以 dry-run 代替公开发行。
- Windows 原生输入法 / 渲染 / 菜单 / 通知、macOS 通知横幅与权限、Dolphin / Nemo 点击、屏幕阅读器、RTL 和低图形能力设备需要对应桌面环境。
- 新 reader 的支持范围以 D54–D60 为准；格式名称不能代表全部压缩变体、文件系统和扩展属性均已覆盖。
- 本次实际生成的 macOS 安装 DMG 使用 APFS；用 RustZIP CLI 测试该安装包时明确返回不支持。换言之，当前 RustZIP 还不能浏览自己的默认 macOS 安装镜像；不能把 HFS+ fixture 通过当作 DMG 全格式完成。

## 剩余开发顺序

1. **DMG APFS**：先补有硬预算和取消的 catalog 遍历、普通文件流式读取、透明压缩与安全链接策略，再用本项目安装 DMG及独立样本在三平台验证。不得将当前无限制的候选直接注册。
2. **WIM LZX / LZMS**：按许可证和不可信输入限制接入分块解码，使用 Windows / wimlib 生成的独立样本覆盖多块、损坏及取消；评估 solid ESD、分卷和特殊属性时分别记录支持边界。
3. **NSIS 自定义 bzip2 与旧版本**：提供有界流式 decoder 和独立 solid / non-solid 样本；继续禁止执行安装脚本。扩展标准 LZMA properties 和大于现有限额的场景前需重新核算资源预算。
4. **发布与平台验收**：在当前改动进入可验证的远端提交后重跑三平台 CI、Fuzz、Release dry-run；配齐真实签名凭据后再演练更新、安装、公证与公开发布。Windows 原生交互、macOS 通知和 Linux Dolphin / Nemo 需对应桌面设备。
5. **性能及可访问性**：补真实大文件 / 百万条目跨平台基线、辅助技术和 RTL 验收。直接在 ZIP 原文件原地改写仍需按 D53 先设计恢复日志并新增决策。
