# 第二阶段：三平台验收

开始日期：2026-10-28。以提交 `d5bf75d` 为初始基线，验收时记录实际代码版本、环境、命令与结果；已有历史验收不能自动代表本轮最新代码通过。

## 工作清单

1. 手动运行已配置的三平台 CI，检查每个任务和实际提交；GUI job 包含 macOS、Windows 和 Ubuntu/Xvfb；失败时定位并修复可在当前工作区处理的问题。手动验收不改变自动触发策略。
2. 在隔离的 macOS 测试环境验证打包、签名、启动、格式关联声明和钥匙串；通知、Finder 和真实用户交互按实际证据分别记录。
3. 检查 Windows 安装器、注册表和 CI 覆盖；具备 Windows 原生环境时验证构建、安装 / 卸载、关联菜单、通知、钥匙串、输入法与渲染。自动化结果不代替真实交互。
4. 基于已有 Ubuntu 验收记录，尽可能在可用 Linux 桌面验证 Dolphin / Nemo；不能运行的桌面环境标记待验收。
5. 修复验收发现的问题并同步产品、发布和进度文档，运行适用的格式检查、Clippy 与测试。

## 执行记录

- 手动 CI [40662760211](https://github.com/yuhuotech/rustzip/actions/runs/40662760211) 测试提交 `d5bf75d`：macOS GUI、macOS 核心和 Ubuntu 核心通过；Windows GUI / 核心均在链接时报 `LNK1181: cannot open input file 'zlib.lib'`；Ubuntu 基准因 `unable to find library -lacl` 失败。该结果不包含之后的工作区改动。
- CI 基准任务补用已有原生依赖 action；Windows vcpkg 设置静态 triplet 以避免重复链接 LZMA，并为 `libarchive2-sys` 适配 zlib 库名称。修复尚未在 Windows runner 完成。
- 手动 CI [40674063269](https://github.com/yuhuotech/rustzip/actions/runs/40674063269) 测试提交 `8c7ea55`：Windows LZMA 导入符号未解析。
- 手动 CI [40679232598](https://github.com/yuhuotech/rustzip/actions/runs/40679232598) 测试提交 `9f7f9cb`：加入 vcpkg LZMA 导入库后与 Rust 静态 LZMA 实现出现 LNK2005 重复符号，Windows GUI / 核心测试失败，安装器步骤未执行。
- 手动 CI [40678318735](https://github.com/yuhuotech/rustzip/actions/runs/40678318735) 测试提交 `115bdba`：vcpkg 缓存目录未初始化，Windows 依赖准备提前失败；基准、macOS 与 Ubuntu 项通过。
- 手动 CI [40683032301](https://github.com/yuhuotech/rustzip/actions/runs/40683032301) 测试提交 `2bcdca5`：Windows 测试发现重复 LZMA 符号；随后切换静态 triplet 时，依赖安装完成但 zlib 兼容脚本未识别静态库文件名，Windows 编译未启动。
- CI 当前使用 vcpkg `x64-windows-static-md` 静态 triplet，按上游实际库名将 zlib `zs.lib` 映射为 `zlib.lib`；切换 triplet 后会清除旧 `libarchive2-sys` 构建缓存。
- 手动 CI [40688806047](https://github.com/yuhuotech/rustzip/actions/runs/40688806047) 的 Windows 核心编译通过，但 RPM gzip fixture 未解压。定位到 `libarchive2-sys` 不跟踪 vcpkg triplet 环境变量，Cargo 缓存复用了旧 CMake 检测结果；action 现加入针对该 crate 的 `cargo clean`，强制按当前依赖重新检测。
- 手动 CI [40694889926](https://github.com/yuhuotech/rustzip/actions/runs/40694889926) 的 Windows 编译通过，普通核心测试、核心 Clippy 及 GUI 测试通过；Inno Setup 编译因 runner 缺少 `ChineseSimplified.isl` 失败，改为随源码固定这份 MIT 翻译后继续验证。安装器随后成功编译、安装并卸载，但冒烟脚本把卸载后无值且无子项的空 `Applications\\rustzip.exe` 键误判为残留；脚本已改为检查注册值和子键，不将空键视为有效关联。
- Inno Setup CI 编译发现 runner 缺少可选的 `ChineseSimplified.isl`。仓库现在固定带入 MIT 翻译文件及许可证，并让安装器从仓库路径读取；Windows 冒烟已验证编译和安装。
- Windows 安装器包含 vcpkg 原生库许可证、格式关联和条件化 RAR 关联；手动 CI 冒烟已在 runner 上编译安装包、安装、核对关联和默认程序、卸载并确认清理。
- macOS 27.0.1 / Apple Silicon：使用隔离配置和测试数据构建 ad-hoc 签名的 `.app` / DMG；默认和 `RZ_WITH_RAR=1` 两种模式的 plist、签名和 DMG 校验通过。默认包没有 RAR 关联与许可证，RAR 测试包有两者；只读挂载 RAR DMG 后再次检查其中应用签名、关联和许可证并卸载。启用 feature 的无头 GUI 用例实际打开 RAR fixture 并验证预览限制提示。
- macOS 真实钥匙串读写测试使用唯一临时服务名和索引，完成写入、读取、列表、删除，并确认测试索引清理。该测试复用生产后端，但未修改用户原有 `RustZIP` 服务的索引。
- macOS 原生窗口打开含中文文件名的 ZIP，搜索框通过辅助功能接口写入中文并显示匹配项；这不等同于拼音 / 五笔输入法验收。实测发现设置开关缺少辅助名称，修复后再次检查，六个开关均显示对应名称。随后无头 GUI 测试复现开关无法用键盘切换，修复后验证开关点击、空格、回车与行点击均各切换一次。
- macOS 任务通知初次实测触发系统“Where is use_default?”应用选择窗口；根因是通知依赖的默认应用查找。已显式指定 RustZIP bundle ID，复测归档测试完成时未再出现该窗口；未捕获到系统通知横幅，实际投递、权限与关闭静默仍待验证。
- Linux 默认 / RAR 两种打包模式通过隔离的脚本结构测试：使用虚构可执行文件检查 feature 参数、生成的桌面 / Nemo 声明与许可证。该测试不构建或运行 Linux 二进制，不替代真实桌面菜单验收。
- macOS Launch Services：`/tmp` 下的包被标记为不可启动，不能作为“打开方式”候选。将两种包复制到工作区隔离目录并使用独立测试 bundle ID 注册后，ZIP 候选包含两种测试包，RAR 候选只包含启用 RAR 的测试包；ZIP 原默认程序保持不变。测试后注销两个临时 bundle，查询确认候选项已清理。该结果验证 Launch Services 查询，尚未逐项点击 Finder 菜单。
- 本轮工作区通过默认与 all-features 工作区测试、全工作区两种 Clippy 检查及格式检查；按定义忽略的真实钥匙串测试已在 macOS 单独运行并通过。CI YAML 解析和 macOS / Linux 打包脚本语法检查通过。
- 手动 CI [40705566104](https://github.com/yuhuotech/rustzip/actions/runs/40705566104) 测试提交 `e54dcdd` 六个任务全部通过，包括 Windows 核心 default / all-features 测试、Windows GUI 测试和安装 / 注册 / 卸载冒烟。
- Windows runner 实际编译了静态 libarchive、GUI 和 Inno 安装器，并验证了注册表默认值未变、安装关联、卸载后的关联清理。首次 Windows Release 安装器构建约 12 分钟；vcpkg 包缓存命中后的依赖准备约 30 秒。
- 最终验收分支提交 `30c4201` 的手动 CI [40721476174](https://github.com/yuhuotech/rustzip/actions/runs/40721476174) 六个任务全部通过：Windows 核心 default / all-features 测试、Windows GUI 测试和安装器冒烟均成功，macOS、Ubuntu 和基准也通过。
- 最终 HEAD `30c4201` 的 Windows vcpkg revision 输出和提前缓存保存配置得到实际运行验证：core 依赖准备约 62 秒，GUI 约 30 秒。另在验收分支的后续 Windows 修复提交 `e54dcdd` 上再次通过 GUI / Inno Setup 安装器冒烟，确认安装、默认关联保护和卸载清理。
- 最新跨平台 CI run [40914277975](https://github.com/yuhuotech/rustzip/actions/runs/40914277975) 对提交 `37f7d26`：Ubuntu core、macOS core / GUI、Windows GUI / installer smoke 和基准通过；Windows core 因 checkout 将 CHM HTML fixture 的 LF 转为 CRLF 而失败。已在 `da02032` 为该 fixture 增加 `.gitattributes` 固定 LF，最新提交的三平台 CI 仍待复跑。
- CI [40920442634](https://github.com/yuhuotech/rustzip/actions/runs/40920442634) 对提交 `da02032`：Ubuntu / macOS core、macOS GUI、Windows GUI / 安装器及基准通过；Windows core 暴露便携 ZIP 权限受 runner 平台默认值影响（`0o100664`，预期 `0o100644`）。ZIP 写入器现明确标注 Unix creator 和 ZIP / tar 兼容权限（目录 `0755`、普通文件 `0644`），大条目分块路径也已固定写入该权限。Linux 安装器隔离安装 / 卸载冒烟已在本机配合 GNU `install` 通过，并已加入 Ubuntu core CI；修复后的三平台 CI 待运行。
- 最新 CI [40926953354](https://github.com/yuhuotech/rustzip/actions/runs/40926953354) 对提交 `0d43a20` 的六个 job 全部通过。Windows core default / all-features、Windows GUI 与安装器安装 / 注册 / 卸载冒烟通过；Ubuntu core 的 Linux Dolphin / Nemo / Nautilus 安装卸载冒烟通过；macOS core default / all-features、GUI 和基准通过。便携 ZIP 文件权限回归在 Windows runner 通过。
- CI [40931063454](https://github.com/yuhuotech/rustzip/actions/runs/40931063454) 对提交 `6659c72` 的七个 job 全部通过，首次验证 Ubuntu GUI runner 成功。Linux runner 安装 GPUI 的 X11 / Wayland / Vulkan / 字体依赖，并在 Xvfb 软件渲染下完成 GUI Clippy 与测试；Windows / macOS GUI 以及三平台 core default / all-features 和基准均通过。
- CI [40935152907](https://github.com/yuhuotech/rustzip/actions/runs/40935152907) 对提交 `f579e8d` 的七个 job 全部通过，包含新的图片预览像素上限回归、Ubuntu/Xvfb GUI、Windows GUI / 安装器以及三平台 core default / all-features。
- CI [40939090908](https://github.com/yuhuotech/rustzip/actions/runs/40939090908) 对提交 `ac18219` 的七个 job 全部通过；Windows、macOS、Ubuntu/Xvfb 的 GUI Clippy 与 GUI all-features 测试通过，RAR 预览回归在三平台构建中通过。三平台 core default / all-features 与 Windows 安装器冒烟通过。
- 本机全工作区默认 / all-features 测试通过自动更新交接用例；系统程序由后台任务启动，签名验证失败时不会调用启动器。当前没有正式签名 Release 可供端到端安装验收，真实安装流程仍待三平台 Release 演练。
- 最终工作区默认及 all-features 的格式检查、Clippy、全工作区测试均通过；每套测试有三项按环境条件忽略。Windows 和 macOS 的原生编译及桌面测试也在最终 CI 通过。
- 2026-10-30：macOS Finder Sync 扩展已加入 app bundle。macOS 27.0.1 ad-hoc 测试包在 Finder 的 ZIP 右键菜单显示 RustZIP 压缩 / 解压选项；压缩入口打开 GUI 创建窗口，解压结果 BLAKE3 与 ZIP 内条目一致。扩展使用本机 PlugInKit 临时注册后已撤销；正式 Developer ID / 公证包及首次启用流程仍待验收。
- 2026-10-30：Release dry-run [40982574540](https://github.com/yuhuotech/rustzip/actions/runs/40982574540) 四个平台常规包构建、签名与汇总验签全部成功；RAR 包三平台构建 / 检查以及 Windows RAR 安装、关联、卸载冒烟通过。macOS Intel 常规打包在新 runner 成功。公开发布步骤按 dry-run 规则跳过。
- 2026-10-30：跨平台 CI [40982559242](https://github.com/yuhuotech/rustzip/actions/runs/40982559242) 对 Finder Sync 与安装日志修复提交的七个 job 全部通过，包含 Windows GUI all-features、Windows 普通安装器冒烟、三平台 core default / all-features、macOS / Ubuntu GUI 和基准。
- 2026-10-30：最新 CI [40994222397](https://github.com/yuhuotech/rustzip/actions/runs/40994222397) 对提交 `0d58852` 的七个 job 全部通过；新增 macOS Quick Look ZIP 安全 smoke 通过，Windows GUI / 安装器、三平台 core 和其余 GUI / 基准均通过。
- 2026-10-30：CI [41003132351](https://github.com/yuhuotech/rustzip/actions/runs/41003132351) 对提交 `adfa799` 的七个 job 全部通过：Windows / macOS / Ubuntu 核心（含 all-features）、三平台 GUI、Windows 安装器冒烟、macOS Quick Look 安全 smoke 与基准均成功。
- 2026-10-30：CI [41010338749](https://github.com/yuhuotech/rustzip/actions/runs/41010338749) 对提交 `171174b` 的七个 job 全部通过：三平台 core default / all-features、三平台 GUI、Windows 安装器冒烟、macOS Quick Look 安全 smoke 与基准均成功；新增 NSIS sniffer 和本地化错误提示通过 Windows / macOS / Ubuntu 编译测试。
- 2026-10-30：CI [41026008129](https://github.com/yuhuotech/rustzip/actions/runs/41026008129) 对提交 `fabff8b` 的七个 job 全部通过：三平台 core default / all-features、三平台 GUI、Windows 安装器冒烟、macOS Quick Look 安全 smoke 与基准均成功；新增 ZIP 文件名编码设置和持久化回归通过三平台 GUI。
- 2026-10-30：Release dry-run [41005419057](https://github.com/yuhuotech/rustzip/actions/runs/41005419057) 对当前 `db0d14e` 成功：macOS arm64 / x86_64、Linux x86_64、Windows x86_64 常规包构建签名通过；四个 RAR 包构建检查通过，Windows RAR 安装 / 关联 / 卸载冒烟通过；汇总 manifest 验签和两个 bundle 上传成功。`publish` 按 dry-run 条件跳过。
- 2026-10-30：当前 HEAD `67b4c2e` 的 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 七个 job 全部成功：四平台常规包构建 / 签名、四平台 RAR 包与许可证检查、Windows RAR 安装 / 关联 / 卸载、manifest 汇总验签和两个 bundle 上传通过；`publish` 按 dry-run 条件跳过。

## 最新远端验收

- 2026-10-31 提交 `4531ed4`：CI [41163708768](https://github.com/yuhuotech/rustzip/actions/runs/41163708768) 七个 job 全部通过，包含 macOS Quick Look 安全 smoke、Windows GUI 和安装器冒烟。Release dry-run [41163708636](https://github.com/yuhuotech/rustzip/actions/runs/41163708636) 四个平台常规 / RAR 包、汇总验签和 bundles 上传全部成功；publish 跳过。
- 2026-10-31 修补版提交 `ac2a230`：Fuzz [41160867029](https://github.com/yuhuotech/rustzip/actions/runs/41160867029) 八个目标全部通过；Release dry-run [41160892957](https://github.com/yuhuotech/rustzip/actions/runs/41160892957) 四平台构建、RAR 包校验和汇总验签通过，publish 跳过。CI [41160892935](https://github.com/yuhuotech/rustzip/actions/runs/41160892935) 的 macOS / Ubuntu、Windows core、benchmark、Windows GUI Clippy / 测试和安装器 smoke 均成功；安装器步骤耗时 14 分 29 秒，GUI job 当时仍在保存 Cargo 缓存。该结果早于本次 Quick Look 原生视图改动。
- 2026-10-31：`rust_h264` bounds patch 又覆盖 intra 16×16 路径，源代码回归、helper process test、完整 workspace Clippy / tests 及 30 秒 ASan 通过。上游库完整测试 205 项 + 6 项文档测试通过，2 项慢速测试忽略。后续 remote Fuzz / CI / Release run 待提交后验收。
- 2026-10-31 最新修补版 `62d5ccd`：CI [41157889872](https://github.com/yuhuotech/rustzip/actions/runs/41157889872) 及 Release dry-run [41157889524](https://github.com/yuhuotech/rustzip/actions/runs/41157889524) 仍在进行，Fuzz [41157872841](https://github.com/yuhuotech/rustzip/actions/runs/41157872841) 排队。每日 schedule 02:07 UTC 未观察到事件，workflow 处于 active。
- CI [41154082845](https://github.com/yuhuotech/rustzip/actions/runs/41154082845) 对 `0a5c460` 七个 job 全部通过；Release dry-run [41154738556](https://github.com/yuhuotech/rustzip/actions/runs/41154738556) 对 `0a5c460` 的四平台常规包、RAR 验证包、Windows 安装冒烟、manifest 验签和 artifact 上传通过，publish 跳过。Fuzz run [41154063015](https://github.com/yuhuotech/rustzip/actions/runs/41154063015) 找到 H.264 8×8 intra 越界；已在本机 vendor patch 中修复并加入 renderer/helper regression，更新版 runner 待 push。
- 2026-10-30：提交 `b32d77d` 的 CI [41097739228](https://github.com/yuhuotech/rustzip/actions/runs/41097739228) 七个 job 全部通过：三平台 core / GUI 和 benchmark 均成功；RAR 多条目累计输出上限回归在三平台 all-features core 测试通过。
- 2026-10-30：提交 `9deb72c` 的 Release dry-run [41090713832](https://github.com/yuhuotech/rustzip/actions/runs/41090713832) 的所有构建、RAR 验证、汇总验签和 artifact job 均通过：macOS arm64 / x86_64、Linux x86_64、Windows x86_64 常规与 RAR 包均成功，Windows RAR 安装 / 卸载及 macOS Quick Look 注册 smoke 通过；`publish` 按 dry-run 规则跳过。
- 2026-10-30：前一轮 CI [41084058660](https://github.com/yuhuotech/rustzip/actions/runs/41084058660) 的 core default lint 找到未按 `rar` feature 条件编译的 `Read` 导入，已修正。提交 `b5c2bd4` 的 CI [41086116233](https://github.com/yuhuotech/rustzip/actions/runs/41086116233) 七个 job 全部通过：Ubuntu / macOS / Windows core、三平台 GUI 和 benchmark 均成功。新增对比回归覆盖加密 7z 独立密码、RAR、CHM、ISO 9660、LHA/LZH、RPM 与 XAR；Windows GUI 安装器 smoke 通过。
- 2026-10-30：提交 `6fd8e65` 的 Release dry-run [41079816983](https://github.com/yuhuotech/rustzip/actions/runs/41079816983) 成功：四平台常规包与 RAR 包验证、Windows RAR 安装 / 关联 / 卸载冒烟、汇总验签和 artifacts 上传均通过；`publish` 按 dry-run 规则跳过。Fuzz run [41079856580](https://github.com/yuhuotech/rustzip/actions/runs/41079856580) 的五个目标及受控 crash artifact smoke 均通过。
- Fuzz workflow 定时验收仍未通过：13:37 UTC 与 workflow 重新注册后的 16:53、17:35 UTC 探测均没有生成 `schedule` run。workflow API 状态为 active，长期 cron 已恢复 13:37 UTC；手动 dispatch 不能代替定时验收。

## 未完成的验收

- Windows CI 与安装器自动化验收通过；原生 GUI 的输入法、渲染、钥匙串、通知和人工交互仍待验收。
- macOS Finder Sync 右键菜单已实机验收；ZIP Quick Look 原生列表在 macOS 27.0.1 当前源码 ad-hoc 包中已由 Finder 空格键显示条目名、类型、大小和加密锁标记，临时 provider 与 Launch Services 注册已撤销。Release macOS runner 注册 smoke [41043955928](https://github.com/yuhuotech/rustzip/actions/runs/41043955928) 的 arm64 / x86_64 job 通过。正式 Developer ID / 公证包的注册、公证、通知横幅投递与权限、屏幕阅读器及真实输入法仍待验收。
- Dolphin / Nemo 的真实桌面点击、Linux 低图形能力设备、三平台百万条真实归档列表仍待验收。
- 默认安装包仍关闭 RAR。Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 已验证 macOS arm64 / x86_64、Linux、Windows 四个 RAR 测试包及随包许可证，并通过 Windows 安装 / 关联 / 卸载冒烟；正式签名发布及发行策略仍待确认。
- Release dry-run [40950855468](https://github.com/yuhuotech/rustzip/actions/runs/40950855468) 的汇总验签因 runner 缺少 `libacl` 开发库失败；之后在 `assemble` 加入原生依赖安装 action。修复后的 dry-run [40958424663](https://github.com/yuhuotech/rustzip/actions/runs/40958424663) 四个平台构建、签名和汇总验签通过；下载的 bundle 已确认 `update.json` 中有四个平台资产和对应文件名，正式发布仍未执行。
- CI [40958398639](https://github.com/yuhuotech/rustzip/actions/runs/40958398639) 对提交 `7ad1791` 的七个 job 全部通过：macOS / Ubuntu / Windows core、三平台 GUI、基准均成功。Windows core job（含 default 与 all-features）耗时 17 分 37 秒；Windows GUI 与安装器冒烟耗时 18 分 44 秒；macOS core 11 分 35 秒，Ubuntu core 7 分 24 秒。Windows GUI job 的缓存保存步骤有非致命失败告警，job 最终仍通过。

## 2026-10-31 媒体预览跨平台回归

- 2026-10-31 最新提交 `0a5c460` 的 CI [41154082845](https://github.com/yuhuotech/rustzip/actions/runs/41154082845) 除 Windows GUI 安装冒烟外六项均通过；本地 vendor patch 已补齐 8×8 越界保护并通过 helper 子进程回归，当前待推送和 runner 复验。对应 Release dry-run [41154738556](https://github.com/yuhuotech/rustzip/actions/runs/41154738556) 四平台常规 / RAR 包、Windows 安装、manifest 验签及 bundle 全部通过。
- Release dry-run [41124950235](https://github.com/yuhuotech/rustzip/actions/runs/41124950235) 对音频预览提交 `bac5fc8` 的四平台常规包构建、签名 / manifest 验证和 Windows 安装包构建通过，公开发布按 dry-run 规则跳过。
- CI [41124926982](https://github.com/yuhuotech/rustzip/actions/runs/41124926982) 的其他六个 job 通过；Windows core 的 `helper_scales_huge_page_boxes_before_rasterization` 使 1,000,000×1,000,000 PDF helper 达到既定 10 秒时限。测试样本已改为长边缩放用例，并增加纯函数断言覆盖极大正方形页面的像素缩放；D30 / D50 资源边界保持原值。新提交 runner 复验待完成。
- Fuzz [41124859136](https://github.com/yuhuotech/rustzip/actions/runs/41124859136) 的其他六个 job 通过；新增 `audio_decoder` target 先后找到零采样率和带伪前缀 WAV，使 Symphonia 0.5.5 / 0.6.1 panic。音频现固定到 Symphonia 0.6.1，拒绝未在文件起始处出现的容器签名，并预检 WAV 采样率 / 声道；两个 crash seed 复放及本机 20 秒 / 287,413 次 ASan fuzz 通过。随后视频 target 找到 MP4 box-parser 无进展超时，加入 16,384 次 read / seek 上限及回归 seed，1,000 次 ASan smoke 通过。新代码远端复验待完成。
- Fuzz [41134178445](https://github.com/yuhuotech/rustzip/actions/runs/41134178445) 的 video decoder 随后发现 sample-table panic；当前修复增加 box 范围检查、采样表校验及三个独立回归 fixture。本机 ASan 跑 30 秒完成 61,003 次变异、峰值 RSS 441 MiB，无崩溃；修复后的 CI / Fuzz runner 结果待本次推送后验收。
- 2026-10-31：Rust 1.96.0 本机全工作区 all-features 测试和 Clippy 通过，覆盖更新后的 MP4 renderer、PDF helper、ZIP reflink 编辑与三平台安装清单；远端 Windows / Linux / macOS runner 尚待新提交触发。
- 2026-10-31：Release dry-run [41143963144](https://github.com/yuhuotech/rustzip/actions/runs/41143963144) 四平台常规包、RAR 包、Windows 安装冒烟、manifest 验签和 bundle 上传均通过，publish 跳过。三平台 CI [41143963257](https://github.com/yuhuotech/rustzip/actions/runs/41143963257) 的其余六个 job 通过；Linux install smoke 测试夹具漏复制新增的 reflink MIT 文本，已修正测试夹具并在本机 GNU install 下通过。Fuzz [41143902645](https://github.com/yuhuotech/rustzip/actions/runs/41143902645) 新发现 Symphonia WAV chunk panic；修复已推送，后续 run [41149879866](https://github.com/yuhuotech/rustzip/actions/runs/41149879866) 等待结果。
- Fuzz run [41149879866](https://github.com/yuhuotech/rustzip/actions/runs/41149879866) 在视频目标发现 `rust_h264` 4×4 intra 越界 panic；本机修复后，run [41154063015](https://github.com/yuhuotech/rustzip/actions/runs/41154063015) 又发现 8×8 intra 越界写。vendor bounds fix 现覆盖两种重建路径；原始 MOV crash 已复放并加入 helper 进程和 renderer 回归测试。
- 2026-10-31：将 `rust_h264` 0.5.0 以 MIT OR Apache-2.0 vendor 并补 intra 4×4 / 8×8 帧边界检查后，Rust 1.96 全工作区 all-features Clippy / 测试通过，视频 renderer 单测 8 项和 helper 资源限制 4 项通过；30 秒本机 ASan fuzz 运行 57,036 次、峰值 RSS 457 MiB，无崩溃。新提交的三平台 CI / Fuzz / Release dry-run 待验收。
- 2026-10-31：CI [41149654270](https://github.com/yuhuotech/rustzip/actions/runs/41149654270) 对 `3456321` 的七个 job 全部通过，包括 Windows core all-features、GUI 与安装 smoke。Release dry-run [41151267352](https://github.com/yuhuotech/rustzip/actions/runs/41151267352) 对 `bd59049` 四平台及 RAR 验证包全部通过，publish 跳过；两者都早于 `rust_h264` vendor patch。当前 vendor patch 的 CI [41154082845](https://github.com/yuhuotech/rustzip/actions/runs/41154082845)、Fuzz [41154063015](https://github.com/yuhuotech/rustzip/actions/runs/41154063015) 正在运行。
