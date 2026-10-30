# RustZIP 下一步开发盘点与计划

2026-10-28 的五阶段执行顺序、第一阶段验收和性能记录见 [开发计划与第一阶段验收](phase-1-stabilization.md)；跨平台实测进展与未完成项见 [第二阶段验收](phase-2-platform-acceptance.md)；RAR、并行 bzip2 / gzip 和 ZIP 安全升级见[第四阶段记录](phase-4-core-safety-throughput.md)。本文保留 2026-10-27 的功能盘点快照。

历史盘点日期：2026-10-27。下表记录当时的代码和验收基线；其后 RAR 硬输出上限与预览、bzip2 并行解压、gzip 分块 Deflate、ZIP 9.1.3 升级和 ZIP 大条目并行 Deflate 已完成，最新执行证据见[第四阶段记录](phase-4-core-safety-throughput.md)。

## 1. 结论

RustZIP 的主要归档与桌面主流程已实现。2026-10-30 增加 macOS Finder Sync 压缩 / 解压菜单并在 macOS 27.0.1 ad-hoc 包实机点击验收；ZIP Quick Look 视图扩展已实现，ZIP/ZIP64 安全 smoke 和系统 WebKit 加载通过。本轮验证了临时 `.app` 经 Launch Services 注册并将 Quick Look provider election 设为 use 后，PlugInKit 可发现 provider，`qlmanage` 能启动扩展；Release matrix 新增可本机通过的注册 smoke，Finder 空格键可见内容因桌面捕获不可用仍未验收，临时注册已撤销。本次 GUI 的 ZIP 文件名编码设置支持自动、UTF-8、GBK、Shift-JIS、EUC-KR、Big5 和 Windows-1252；GUI 持久化回归、Rust 1.96 全工作区 all-features 测试、Clippy 及三平台 CI [41026008129](https://github.com/yuhuotech/rustzip/actions/runs/41026008129) 七个 job 均通过。线上官网和数据库健康检查通过，`RZ_STATS_URL` 已配置；同一构建缓存目录内验证设置该变量会触发重编译并嵌入正确地址。最新 Release dry-run [41026007832](https://github.com/yuhuotech/rustzip/actions/runs/41026007832) 的常规和四平台 RAR 构建、安装冒烟与汇总验签均通过；公开发布 job 跳过。最新五目标 fuzz run [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 全部通过。其余主要缺口为 PDF / 音视频应用内预览、与 D11 崩溃保全约束冲突的 ZIP 真正原地增量编辑，以及安全 / 许可门槛尚未满足的 WIM / DMG / NSIS reader。Windows 原生交互、Dolphin / Nemo 实机菜单和正式签名发布仍待验收。虚拟挂载、云端按需解压、WASM 插件和 SFX 仍列为远期需求。

四个曾经占位的侧栏功能——常用位置、密码本、格式转换、压缩包修复——在当前代码中已经实现，不能继续列为“未开发”。转换与修复的符号链接和损坏数据边界见 [侧栏实现记录](sidebar-implementation.md) 与 [决策 D9](decisions.md#d9-左栏工具的数据安全与恢复边界)。

## 2. 功能现状

| 优先级 | 能力 | 当前状态 | 下一步 |
|---|---|---|---|
| P0 | 基础压缩、解压、浏览、加密、任务队列和 CLI | 主路径已完成；CLI 提供 `a / l / x / t / c / r / h`，长操作均支持 Ctrl+C 与进度 | 保持现有安全契约，继续做平台验收与性能验收 |
| P0 | 分卷压缩与多卷解压 | ZIP / 7z 容量设置、缺卷诊断和无覆盖发布已接入；往返、取消、现有卷保护和发布竞态回滚测试通过。Linux arm64 上与 p7zip 26.02 双向互通；Ubuntu 26.04 x86_64 上与 p7zip 26.00 双向互通，Info-ZIP 3.0 创建的 split ZIP 可由 RustZIP 测试和解压 | 扩大到 Windows / macOS 官方 7-Zip、WinZip 等常用工具版本；Info-ZIP `unzip` 不支持本测试的 split ZIP，不能作为读取验收 |
| P0 | LZ4 与 tar.lz4 | 标准 LZ4 Frame 读写、魔数识别、CLI、编辑、转换和 GUI tar.lz4 入口已接入；格式及引擎往返通过，外部 `lz4 -t` 和 `tar -tf` 通过；8 MiB 基准已记录 | 扩大外部样本和性能采样，保留 Frame 格式边界 |
| P0 | ISO、CAB、WIM、DMG、CPIO、AR、LHA、XAR、RPM、DEB、CHM、NSIS 等只读格式 | ISO 9660、CAB、CPIO、AR、LHA/LZH、XAR、RPM 已通过隔离的 libarchive 后端接入；CHM 经独立样本验证，使用纯 Rust reader 支持只读浏览 / 测试 / 提取；WIM、DMG 仅按内容识别并返回不支持；NSIS 现按 PE overlay 的 FirstHeader 签名识别但仍不读取；DEB 按 AR 容器读取。候选审查和限制见[第三阶段记录](phase-3-format-assessment.md) | 为 WIM / DMG / NSIS 继续评估具备流式、取消、硬资源上限和独立样本的后端；NSIS 识别不等于安装器读取；保持 RAR 只读边界 |
| P0 | 压缩参数 | ZIP 压缩级别 / 线程与 7z 字典大小 / 固实块大小已接入 CLI / GUI；7z 并行工作集受内存预算估算限制；自动字典下调、超预算拒绝及固实块结构用例通过 | 无；ZIP 的 deflate 字典不适用 |
| P0 | 7z 多块并行解压 | 多个独立固实块按块调度，支持选择性解压、AES 和取消；按 LZMA2 字典估算并行内存，遇到其他编码或预算不足回退顺序解码。加密、错误密码、损坏块、取消、选择和低预算回退的本机用例通过 | 验证外部工具样本、更多字典与分卷组合，并在三平台和大数据集上对比性能 |
| P0 | bzip2 多线程 | 单文件和 tar.bz2 按块并行压缩；读取通过 `pbz2` 并行解码单流和串接 member，按线程 / 内存预算估算限 worker；压缩与解压均有顺序回退，系统 bzip2 互通用例通过 | 扩大外部样本、损坏 / 取消边界和三平台验收 |
| P0 | 大文件 Deflate | `.gz` / `.tar.gz` 在多线程模式下以最多 900,000 字节块压成有序标准 gzip members；未加密 ZIP 大条目已分块并行 Deflate，通过 Info-ZIP `unzip -t`、引擎往返、取消清理和 BLAKE3 校验。40 / 64 / 128 MiB 每种三轮的中位数见[基准记录](benchmarks/2026-10-29-zip-large-deflate.md) | Windows / macOS 7-Zip、WinZip 外部互通与其他数据类型性能样本；ZIP 加密继续顺序编码 |
| P0 | 校验和 | `rz-app` 流式 SHA-256 / BLAKE3、CLI `rz h` 和最近文件 / 常用位置的 GUI 入口已接入；标准向量、分块进度、取消、非普通文件拒绝及 workspace 回归通过 | 无；保留与归档 CRC 校验的语义区分 |
| P1 | 预览 | 详情栏支持受限图片 / UTF-8 文本；启用 RAR 时单条目预览走有界内存读取，不写临时文件。macOS Quick Look 视图扩展列出 ZIP 目录（中央目录最多 16 MiB / 50,000 项，显示 200 项），不解压内容；安全 smoke 与 `qlmanage` provider 加载通过。临时包经 Launch Services / PlugInKit 注册后 provider 可发现并启动；Finder 可见页面、正式签名包注册仍待验收。PDF / 音视频仍由系统程序打开 | 验收 Finder 空格键可见 UI 和 Developer ID / 公证包注册；仅在资源硬上限与许可证符合 D30 后评估 PDF；再评估音视频预览 |
| P1 | 压缩包内编辑 | 添加、删除、重命名已实现；可用系统程序编辑条目并自动写回。符合条件的单卷、无密码 ZIP 删除 / 重命名 / 条目写回复制原压缩数据，保留归档 / 条目注释、自定义扩展字段和 ZIP64 扩展数据；替换条目时仍保留原注释；自解压前缀、非 UTF-8 名称、加密、分卷及其他操作回退完整重打包；仍非原地增量写入 | 扩大厂商扩展字段和跨平台替换验证；原地增量写入仍需先解决与 D11 崩溃保全的决策冲突 |
| P1 | 完成后动作与通知 | 设置可控解压完成后显示目录；历史记录为本次运行内成功任务提供本次运行结果定位，不持久化结果路径；可选系统通知只发通用成功 / 失败信息，默认关闭。Ubuntu GNOME 开启时实际投递、关闭时静默均已通过 | 验收 macOS / Windows 通知实际投递与设置切换；关机等高影响动作暂不纳入 |
| P1 | 压缩预设 | 极速 / 均衡 / 最小体积、邮件分享（20 MiB 默认分卷）和跨平台兼容预设已接入；自定义分卷、权限规范化用例与 8 MiB 基准通过 | 无；用户指定的分卷大小会直接覆盖预设值 |
| P1 | 重复文件检测 | GUI 压缩前按内容 BLAKE3 检查同大小普通文件，任务可取消并报告进度；发现重复后可继续压缩或取消，不自动删除；超过 100,000 个文件时可明确选择继续 | 扩充大目录压力样本；自动去重另行评估 |
| P1 | 多语言和无障碍 | 十种语言的 381 个界面资源键已实现；八种新增语言各有完整词条包，覆盖及占位符测试通过。十种菜单选项逐项切换测试通过；设置和窗口顶栏共用语言下拉菜单，系统首选语言读取可用，侧栏使用简短本地化标签并保留省略保护 | Windows 原生语言设置、真实屏幕阅读器、阿拉伯语 RTL 整页布局仍待平台验收 |
| P1 | ZIP 文件名编码 | 核心自动识别及 CLI 手动指定已实现；GUI 全局设置可选自动、UTF-8、GBK、Shift-JIS、EUC-KR、Big5、Windows-1252，关闭并重新打开压缩包后生效。十种语言资源均已更新；macOS GUI 设置持久化回归及 Rust 1.96 全工作区 all-features 测试通过 | 本轮三平台 CI 通过后关闭此项平台验证 |
| P1 | 文件关联与系统菜单 | macOS Finder Sync 右键扩展已打入 `.app` 并在 macOS 27.0.1 实际点击验收；用户需启用扩展，Developer ID / 公证包待验收。ZIP Quick Look 扩展已打包；临时包注册后 provider 在 PlugInKit 可见并被 `qlmanage` 启动。Release macOS matrix 新增注册 smoke，本机脚本通过，Finder 空格键可见 UI 仍待验收。Nautilus 50.2.2 Scripts 菜单在 Ubuntu 26.04 / Xvfb 已点击验收；Dolphin / Nemo 和 Windows 注册表菜单的原生点击仍待验收 | 运行新增注册 smoke；验收 Finder 可见页面与正式签名包；验收 Plasma Dolphin、Nemo 和 Windows 菜单 |
| P1 | 持续 fuzz | nightly ASan 工作流覆盖 ZIP recovery、CHM、bzip2、RAR、NSIS。最新手动 run [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 五个 target 全部成功并保存 corpus cache：ZIP recovery 265,079 次、CHM 54,151 次、NSIS 2,639,918 次、RAR 137,390 次、bzip2 5,460 次，无 crash artifact | 03:23、05:53、07:37、09:37 UTC schedule 均未生成 run；cron 改为每日 11:37 UTC，等待推送后重新启用并验证 |
| P1 | 跨平台桌面与打包 | macOS `.app` / DMG arm64 签名与启动通过，尚未公证。Ubuntu 26.04.1 x86_64 的全工作区 all-features 测试与 Clippy 通过（3 项环境条件测试忽略）；Linux tarball 构建、隔离安装 / 卸载、Nautilus 菜单点击通过。真实 Wayland GUI 启动、GNOME Secret Service 唯一标签往返、通知启用投递 / 关闭静默通过。CI [40958398639](https://github.com/yuhuotech/rustzip/actions/runs/40958398639) 的七个 job 全部通过，包括新增对比功能的 GUI、三平台 core 和基准；Windows core 17 分 37 秒、Windows GUI / 安装器 18 分 44 秒。Windows GUI 缓存保存有非致命告警。详细证据见 [第二阶段验收](phase-2-platform-acceptance.md) | Windows 原生输入法 / 渲染 / 钥匙串 / 通知、macOS 公证 / Finder / 通知、Dolphin / Nemo 点击、屏幕阅读器、正式三平台发布流程仍需验收 |
| P1 | 自动更新与 Release | 有安装包签名校验、更新发现和下载后交给系统安装器的流程；自动化用例覆盖验签后才启动、篡改内容拒绝启动及启动失败提示。提交 `fabff8b` 的 dry-run [41026007832](https://github.com/yuhuotech/rustzip/actions/runs/41026007832) 四平台常规包、四平台 RAR 包、Windows RAR 安装卸载、manifest 汇总验签和两个 bundle 上传全部通过，公开发布按 dry-run 规则跳过 | 正式签名 Release 尚未执行；正式签名 / 公证、三平台更新器交接和真实安装流程仍需验收 |
| P2 | 压缩包对比 | 已接入 GUI 已打开标签对比和 CLI `rz diff`；按路径 / 类型识别新增、删除和类型变化，同路径同大小普通文件流式计算 BLAKE3；遵守取消、进度、总输出和压缩比限制。已验证 ZIP、固实 7z 与 tar.gz 对比及目录隐式项处理；单包最多 1,000,000 条目，符号链接目标标记未比较 | 扩展外部格式样本和大归档性能采样；验证加密 7z、RAR 和长尾格式输入 |
| P2 | 虚拟磁盘挂载、云端按需解压、WASM 插件、自解压包 | 尚未实现，仍属远期需求 | 分别评估平台成本、威胁模型与许可后再立项 |

格式范围的依据：产品规格承诺 LZ4 读写及一组只读长尾格式；当前识别枚举已包含 ZIP、tar、gzip、7z、RAR、xz、zstd、bzip2、LZ4、ISO 9660、CAB、CPIO、AR、LHA/LZH、XAR、CHM、WIM、DMG 和 NSIS。CHM 可读；WIM / DMG 仅识别并明确不支持；NSIS 只识别有效 PE overlay FirstHeader，仍不读取。RAR 仍只通过可选 `unrar` 后端只读。

## 3. 分阶段计划

### 阶段 A：跨平台验收与规格收口

1. 补齐 Windows 原生 GUI、中文输入、渲染、长列表和安装器验收；Ubuntu 26.04 的 GUI、全工作区测试、Nautilus 菜单和 tarball 安装已通过；Linux GUI/Xvfb CI 首轮通过，继续补平台人工验收。
2. 在 Windows 实机验证系统钥匙串、注册表菜单和安装 / 升级产物；在 macOS 验收系统钥匙串与通知。Linux 的 Secret Service、通知、Nautilus 菜单及安装 / 卸载已有 Ubuntu 证据；Dolphin / Nemo 仍需实际桌面点击验收。
3. ~~统一自动更新产品规格与决策。~~ 已按 D8 明确为验签后交给系统安装器，不静默替换；正式发布时仍需验证实际安装流程。
4. 为长尾格式建立格式清单：样本、读写档位、加密 / 多卷情况、许可证与依赖、明确的不支持提示。

**退出条件：** 产品规格和决策一致；三平台已支持能力有逐项证据；CI 和发布工作流不把实验项报告为通过。

### 阶段 B：补齐 P0 归档能力

1. ~~实现 ZIP 与 7z 分卷容器的创建、识别、读取和缺卷诊断，并提供 GUI / CLI 操作入口。~~ 往返、缺卷、取消、已有卷保护及逐卷发布竞态回滚通过；Linux arm64 / p7zip 26.02 与 Ubuntu x86_64 / p7zip 26.00 的 ZIP / 7z 双向互通通过，Info-ZIP 3.0 split ZIP → RustZIP 也通过；待 Windows / macOS 常用工具矩阵。首版范围和卷命名见决策 D18。
2. ~~接入 LZ4 与 tar.lz4 读写、格式识别和转换。~~ 格式 / 引擎往返、外部 `lz4 -t`、workspace all-features 编译通过；8 MiB 本机基准已运行。新增依赖许可证已核对。
3. ~~增加格式相关的字典大小、固实块等高级参数。~~ 7z 字典 / 固实块入口已接入，按格式隐藏；参数边界、固实块 / 非固实块、CLI / GUI 创建、workspace 测试和 Clippy 通过。8 MiB 基准矩阵完整运行并校验输出；受限样本仅用于运行路径验收，没有拿来作性能回归对比。
4. ~~补齐文件 SHA-256 / BLAKE3 校验。~~ `rz-app` 单次流式读取同时计算算法、支持取消和进度；CLI `rz h` 与最近文件 / 常用位置的 GUI 操作入口已接入。标准向量、分块进度、取消、普通文件边界、workspace 测试、fmt 与 Clippy 通过；保留 `rz t` 的归档完整性测试语义。
5. ISO 9660、CAB、CPIO、AR、LHA/LZH、XAR 与 RPM 只读实现已接入；RPM 用 bzip2/gzip CPIO 样本验证内置滤镜、CLI 列表/完整性/安全提取，并由 p7zip 26.00 独立测试。第三阶段新增 CHM 只读 reader，以 `rustchm` 生成的独立 fixture 验证目录、LZX 多块读取、取消、上限和路径安全。WIM / DMG 仅明确识别不支持；NSIS 现只识别 PE overlay FirstHeader，reader 仍不满足 D37，普通 `.exe` 不作推测；其余后端候选不满足门槛。详见[第三阶段记录](phase-3-format-assessment.md)。

**退出条件：** 已接入的新格式和分卷操作具备损坏输入、加密适用性、缺卷、取消与路径安全验收；候选不满足安全门槛的承诺格式在嗅探、界面提示和文档中保持明确不支持，不计入已实现能力；新参数受全局内存预算约束。

### 阶段 C：完成高频 P1 体验

1. ~~实现受限大小的纯文本应用内预览。~~ 64 KiB 白名单 UTF-8 内容在详情栏等宽显示；继续评估 PDF 原生预览，再决定音视频接入方式。
2. ~~将“邮件分享”预设建立在分卷能力之上；实现跨平台兼容预设。~~ 邮件分享对 ZIP / 7z 采用 20 MiB 默认分卷，用户可覆盖；跨平台预设规范化源权限并省略 7z Windows 文件属性。
3. ~~为重复文件检测明确判定规则并先提示用户。~~ 已按同大小 + 内容 BLAKE3 找出重复组，确认前不压缩；目录上限与越界后的继续提示已覆盖；自动去重仍需单独评估其安全边界。
4. ~~接入可选系统通知、解压完成后定位和任务记录中的本次运行结果定位。~~ Ubuntu GNOME 通知启用 / 关闭实测通过；继续验收 macOS / Windows 的实际投递和设置切换。
5. ~~完成中文 / 英文界面资源迁移。~~ 已扩展到十种语言、364 个界面资源键；系统语言识别、语言切换、资源覆盖和侧栏标签宽度在 macOS / Ubuntu 通过。Windows 语言 API、屏幕阅读器及阿拉伯语 RTL 整页布局仍待平台验收。
6. ~~补齐 Linux Nautilus 入口并实际点击验收。~~ Ubuntu 26.04 / Xvfb 中 Nautilus 50.2.2 显示并点击了压缩 / 解压 Scripts 菜单；解压的两个 Unicode 文件逐字节一致，安装器在 PATH 不含安装目录时仍能启动 RustZIP，安装 / 卸载通过。后续验收 Dolphin / Nemo 和 Windows 菜单，再评估 Finder 右键及 Quick Look。

**退出条件：** 主流程可由键盘操作；错误后可继续使用；预览有明确大小限制；通知和完成动作不阻塞 GUI 线程；三平台行为差异对用户有清晰说明。

### 阶段 D：CLI 对齐与远期能力筛选

1. ~~让 CLI 经 `rz-app` 调用核心能力。~~ `rz-app::cli` 提供创建、列表、解压和完整性测试门面；CLI crate 已移除 `rz-engine` / `rz-formats` 直接依赖，并通过共享应用 API 接收取消与进度。
2. ~~为格式转换和压缩包修复增加 CLI 命令。~~ `rz c` / `rz convert` 与 `rz r` / `rz repair` 已经由 `rz-app::cli` 调用流式转换 / 恢复；支持源与目标分别输入密码、隐藏提示 / 密码文件、Ctrl+C、限频进度和修复问题报告。转换 / 修复暂不支持分卷输出。列表枚举也已接入各格式的可取消进度回调。
3. ~~实现首批压缩包对比。~~ GUI 可选择另一个已打开的标签，结果从任务面板或历史记录查看；CLI `rz diff` 支持共用或分开的归档密码。虚拟挂载、云读取、插件和 SFX 仍按许可、威胁模型和平台成本分别评估。

**退出条件：** GUI 与 CLI 的能力边界有明确说明；CLI 只通过应用层使用核心；每项远期能力有单独的安全与维护成本评估。

## 4. 贯穿各阶段的验收

- 延续 [稳定性提升计划](testing-stability-plan.md) 与 [S1 执行报告](testing-s1-report.md)：先完成仍待执行的 GUI 可恢复性和跨平台格式 / 系统验收，再做压力、性能与发布产物检查。
- 所有新格式、分卷、转换、修复和编辑都验证取消、损坏输入、路径边界、同名冲突、旧目标保全及临时文件清理。
- 触碰压缩 / 解压热路径时跑基准并与同机基线比较；不能用单次吞吐结果替代正确性。
- 2026-10-30 已对线上 `https://rustzip.vercel.app` 完成只读验收：官网、`/formats`、`/privacy` 和 `/api/health` 返回 HTTP 200，健康接口报告数据库正常；GitHub `RZ_STATS_URL` 已设为生产遥测地址。尚无正式 Release，现有安装包不会自动改变；未来发布的客户端仍受首次告知及用户选择控制。

## 5. 建议的首批实施顺序

1. 优先完成 Windows 原生 GUI、安装器、文件关联及注册表菜单验收，并补上 Windows CI / 发布证据。
2. 完成 macOS 系统钥匙串与通知验收；验证 Dolphin / Nemo 菜单点击和 Linux GUI CI。
3. ~~评估 WIM、DMG、CHM、NSIS 等只读格式候选并按安全门槛接入。~~ CHM 已接入只读 reader；WIM / DMG 魔数分流到明确不支持；NSIS 已加 PE overlay FirstHeader 识别，但读取仍待符合 D37 的后端。其余格式只有后端满足流式、取消、资源上限和独立样本要求后再接入，证据见[第三阶段记录](phase-3-format-assessment.md)。
4. 评估 PDF 原生预览的硬资源上限；音视频继续调用系统程序，直到资源和平台成本明确。
5. 完成 Windows 安装器和三平台 release 工作流演练，验证更新的验签、下载与安装器交接后再发布首个正式版本。

没有为各阶段给出日历工期：长尾格式范围、Windows / Linux 设备与打包环境仍需先确认，当前估时会产生较大偏差。

## 6. 继续开发时的具体边界

- 7z 多块并行按固实块调度，每个工作线程使用独立输入流；对于 COPY、LZMA2 和 AES 组合，按最大字典加固定开销估算线程数。默认预算为 256 MiB，该估算不包含整个进程内存和全部归档元数据，不是硬资源限制。
- WIM / DMG / NSIS 的候选版本、许可证、API 和资源边界见[第三阶段评估](phase-3-format-assessment.md)；未通过安全门槛的候选不进入依赖或格式注册表。CHM 的上游 MIT reader 经本地范围读取 / 取消补丁后接入，硬资源上限及 fixture 也记录在该阶段文档。
- RAR 使用 `rz-ffi` 的 UnRAR 数据回调：所选输出按声明大小和剩余 `max_total` 限量写入单条目临时文件；未选条目不写盘，测试流支持取消。`ArchiveReader::open` 整条目兼容读取限制为 64 MiB；RAR 图片 / 文本预览使用限额内存读取。四平台 RAR 测试包和 Windows 安装冒烟已由 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 验证；回归与边界见 D38，正式签名发布仍待验收。
- 2026-10-27 本机 `cargo check -p rz-app -p rustzip-gui` 通过；2026-10-28 增加预览与更新交接定向测试，正式安装器实机验收仍未完成。
- 7z 顺序遍历改为逐块检查解码器的停止信号，避免预览结束后继续处理后续块；`cargo check -p rz-app -p rustzip-gui` 通过。`RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 跑完整套矩阵并通过基准自身的输出检查；单次 8 MiB 样本不能证明无性能回归，同机基线和多次采样仍待补齐。
- 7z 多块路径的本机 `parallel` 与 `selective` 两组测试共 14 项通过；新增 8 MiB 多块基准中单线程约 228 MiB/s、多线程约 846 MiB/s。该数字仅为单次小样本，不能代替性能回归结论。
- bzip2 多线程压缩使用标准连续流；3.1 MB 单文件由本机系统 `bzip2 -dc` 逐字节解码通过，tar.bz2 由系统 `bsdtar` 解出并逐字节比对通过，低预算回退往返通过，注入输出错误后工作线程正常退出。8 MiB 基准单线程约 21 MiB/s、多线程约 161 MiB/s；单次样本不能代替回归结论。
- 本机 `cargo test -p rz-formats -p rz-engine -p rz-app --all-features` 通过，包括可选 RAR 用例；两个要求非 root Unix 账户的既有用例被忽略。本机针对涉及的六个包运行 `cargo clippy --all-targets -- -D warnings` 通过，未覆盖其余工作区包或其他平台。
- ZIP 压缩数据复用覆盖删除、重命名和单文件写回（包括 UTF-8 名称、隐式目录）；D42 扩展到归档注释、条目注释、自定义扩展字段和 ZIP64 扩展数据。取消、损坏数据、外部替换、密码回退、跨进程编辑和中途终止用例在本机通过。该快路径继续生成新归档、校验后替换，不是原地增量写入。`RZ_BENCH_MB=8 cargo bench -p rz-app --bench zip_edit` 对照完整重打包并按文件哈希验证；样本不足以证明普遍性能。ZIP 大条目分块 Deflate 在 40 / 64 / 128 MiB 每尺寸三轮，完整结果见基准记录。
