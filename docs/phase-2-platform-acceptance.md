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

## 未完成的验收

- Windows CI 与安装器自动化验收通过；原生 GUI 的输入法、渲染、钥匙串、通知和人工交互仍待验收。
- macOS Finder Sync 右键菜单已实机验收；ZIP Quick Look 视图扩展已打包 / 签名，`qlmanage` 启动 provider 并确认 WebKit 页面加载。2026-10-30 Finder 空格键实测显示系统通用 ZIP 图标，PlugInKit 注册列表没有 RustZIP Quick Look 扩展，因此 provider 可见内容尚未通过；正式 Developer ID / 公证包的扩展注册、公证、通知横幅投递与权限、屏幕阅读器及真实输入法仍待验收。
- Dolphin / Nemo 的真实桌面点击、Linux 低图形能力设备、三平台百万条真实归档列表仍待验收。
- 该阶段的默认安装包不启用 RAR。之后 D38 已为 RAR 增加可取消且受 `max_total` 限制的逐块输出；正式发行仍需在三平台验证启用 RAR 的构建、许可证和发行策略，默认包继续关闭 RAR。
- Release dry-run [40950855468](https://github.com/yuhuotech/rustzip/actions/runs/40950855468) 的汇总验签因 runner 缺少 `libacl` 开发库失败；之后在 `assemble` 加入原生依赖安装 action。修复后的 dry-run [40958424663](https://github.com/yuhuotech/rustzip/actions/runs/40958424663) 四个平台构建、签名和汇总验签通过；下载的 bundle 已确认 `update.json` 中有四个平台资产和对应文件名，正式发布仍未执行。
- CI [40958398639](https://github.com/yuhuotech/rustzip/actions/runs/40958398639) 对提交 `7ad1791` 的七个 job 全部通过：macOS / Ubuntu / Windows core、三平台 GUI、基准均成功。Windows core job（含 default 与 all-features）耗时 17 分 37 秒；Windows GUI 与安装器冒烟耗时 18 分 44 秒；macOS core 11 分 35 秒，Ubuntu core 7 分 24 秒。Windows GUI job 的缓存保存步骤有非致命失败告警，job 最终仍通过。
