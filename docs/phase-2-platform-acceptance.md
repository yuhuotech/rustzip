# 第二阶段：三平台验收

开始日期：2026-10-28。以提交 `d5bf75d` 为初始基线，验收时记录实际代码版本、环境、命令与结果；已有历史验收不能自动代表本轮最新代码通过。

## 工作清单

1. 手动运行已配置的三平台 CI，检查每个任务和实际提交；失败时定位并修复可在当前工作区处理的问题。手动验收不改变自动触发策略。
2. 在隔离的 macOS 测试环境验证打包、签名、启动、格式关联声明和钥匙串；通知、Finder 和真实用户交互按实际证据分别记录。
3. 检查 Windows 安装器、注册表和 CI 覆盖；具备 Windows 原生环境时验证构建、安装 / 卸载、关联菜单、通知、钥匙串、输入法与渲染。自动化结果不代替真实交互。
4. 基于已有 Ubuntu 验收记录，尽可能在可用 Linux 桌面验证 Dolphin / Nemo；不能运行的桌面环境标记待验收。
5. 修复验收发现的问题并同步产品、发布和进度文档，运行适用的格式检查、Clippy 与测试。

## 执行记录

- 手动 CI [40662760211](https://github.com/yuhuotech/rustzip/actions/runs/40662760211) 测试提交 `d5bf75d`：macOS GUI、macOS 核心和 Ubuntu 核心通过；Windows GUI / 核心均在链接时报 `LNK1181: cannot open input file 'zlib.lib'`；Ubuntu 基准因 `unable to find library -lacl` 失败。该结果不包含之后的工作区改动。
- CI 基准任务补用已有原生依赖 action；Windows action 为 `libarchive2-sys` 要求的 `zlib.lib` 提供 vcpkg `z.lib` 导入库别名，并将 vcpkg DLL 目录加入运行时 PATH。修复尚未在 Windows / GitHub Actions 重跑，不能视作通过。
- 手动 CI [40674063269](https://github.com/yuhuotech/rustzip/actions/runs/40674063269) 测试提交 `8c7ea55`：macOS 两项、Ubuntu 核心及基准通过；Windows 依赖准备和 Clippy 通过，但测试链接阶段报 `__imp_lzma_stream_decoder` / `__imp_lzma_alone_decoder` 未解析，安装器步骤未执行。日志显示 Rust 静态 `lzma.lib` 遮蔽了 vcpkg 动态导入库；本轮增加独立导入库名称及 vcpkg 二进制缓存，修复尚待新一轮 Windows 验证。
- Windows 安装器补充 vcpkg 运行时 DLL 与许可证、现有长尾格式关联以及条件化的 RAR 关联；新增静态一致性测试和手动 CI 中的隔离安装 / 卸载冒烟脚本。脚本检查默认程序、文件、Open With 和注册表清理，但尚未在 Windows 执行；DLL 名称、安装器编译、加载和真实交互仍需原生验证。
- macOS 27.0.1 / Apple Silicon：使用隔离配置和测试数据构建 ad-hoc 签名的 `.app` / DMG；默认和 `RZ_WITH_RAR=1` 两种模式的 plist、签名和 DMG 校验通过。默认包没有 RAR 关联与许可证，RAR 测试包有两者；只读挂载 RAR DMG 后再次检查其中应用签名、关联和许可证并卸载。启用 feature 的无头 GUI 用例实际打开 RAR fixture 并验证预览限制提示。
- macOS 真实钥匙串读写测试使用唯一临时服务名和索引，完成写入、读取、列表、删除，并确认测试索引清理。该测试复用生产后端，但未修改用户原有 `RustZIP` 服务的索引。
- macOS 原生窗口打开含中文文件名的 ZIP，搜索框通过辅助功能接口写入中文并显示匹配项；这不等同于拼音 / 五笔输入法验收。实测发现设置开关缺少辅助名称，修复后再次检查，六个开关均显示对应名称。随后无头 GUI 测试复现开关无法用键盘切换，修复后验证开关点击、空格、回车与行点击均各切换一次。
- macOS 任务通知初次实测触发系统“Where is use_default?”应用选择窗口；根因是通知依赖的默认应用查找。已显式指定 RustZIP bundle ID，复测归档测试完成时未再出现该窗口；未捕获到系统通知横幅，实际投递、权限与关闭静默仍待验证。
- Linux 默认 / RAR 两种打包模式通过隔离的脚本结构测试：使用虚构可执行文件检查 feature 参数、生成的桌面 / Nemo 声明与许可证。该测试不构建或运行 Linux 二进制，不替代真实桌面菜单验收。
- macOS Launch Services：`/tmp` 下的包被标记为不可启动，不能作为“打开方式”候选。将两种包复制到工作区隔离目录并使用独立测试 bundle ID 注册后，ZIP 候选包含两种测试包，RAR 候选只包含启用 RAR 的测试包；ZIP 原默认程序保持不变。测试后注销两个临时 bundle，查询确认候选项已清理。该结果验证 Launch Services 查询，尚未逐项点击 Finder 菜单。
- 本轮工作区通过默认与 all-features 工作区测试、全工作区两种 Clippy 检查及格式检查；按定义忽略的真实钥匙串测试已在 macOS 单独运行并通过。CI YAML 解析和 macOS / Linux 打包脚本语法检查通过。Windows PowerShell / Inno Setup 脚本未在本机执行。

## 未完成的验收

- 修复后的 CI 尚未推送与重跑；Windows 原生 GUI、安装器、注册表、输入法、钥匙串、通知和安装 / 卸载未通过验收。
- macOS Finder“打开方式”实际展示、公证、通知横幅投递与权限、屏幕阅读器及真实输入法仍未验收。
- Dolphin / Nemo 的真实桌面点击、Linux 低图形能力设备、三平台百万条真实归档列表仍待验收。
- 目前的默认安装包不启用 RAR。正式发行版要求的 RAR 安全边界、许可证与发行策略须在发布前闭环，参见 D35 / D36。
