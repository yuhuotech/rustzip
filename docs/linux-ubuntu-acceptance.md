# Ubuntu 26.04 Linux 验收记录

日期：2026-10-27。目标环境：用户提供的 Ubuntu 26.04.1 LTS、x86_64、GNOME Wayland。工作区与构建输出位于专用 `/tmp/rustzip-linux-e2e-20261026/`、`/var/tmp/rustzip-linux-e2e-target/`；Nautilus 安装与菜单点击使用另一个临时目录。

## 核心和 GUI

- Rust 1.96.0；`cargo fmt --all -- --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `xvfb-run -a env LIBGL_ALWAYS_SOFTWARE=1 cargo test --workspace --all-features --quiet` 通过；GUI 58 项测试全部通过，语言测试覆盖十种菜单选择、364 个资源键、翻译占位符和十种语言下的全部侧栏导航。全工作区有 3 项环境条件测试按定义忽略。侧栏布局测试在最新代码的 macOS 与 Ubuntu 26.04 上都通过。
- `cargo build --all-features -p rz -p rustzip-gui` 和 `cargo build --release -p rustzip-gui` 通过。RustZIP 发布 tarball 成功生成。
- GUI 在真实 GNOME Wayland 会话中启动并保持运行 18 秒；另在 Xvfb 软件渲染环境启动并保持运行 12 秒。Xvfb 输出 Mesa 未提供 DRI3 presentation 的提示；真实 Wayland 启动无错误日志。
- 远端系统 locale 为 `LANG=en_US.UTF-8`。本机 macOS 首选语言为 `zh-Hans-CN`；`sys-locale` 返回的首选标签按主语言码映射到 RustZIP 的简体中文选项。

## 归档与系统集成

- RustZIP 创建的分卷 ZIP 和 7z 均由 p7zip 26.00 测试并解压，内容与源目录逐文件比对一致。
- Info-ZIP 3.0 创建的 split ZIP 和 p7zip 创建的分卷 7z 均由 RustZIP 测试、解压并逐文件比对一致；样本包含中日韩文件名和随机数据。
- RPM 只读 fixture 经 RustZIP `rz l` / `rz t` / `rz x` 列表、完整性测试与安全引擎提取成功；bzip2、gzip 两种载荷的测试覆盖通过。p7zip 26.00 将上游 RPM fixture 识别为 RPM 并独立测试通过。
- Linux tarball 的免 root 安装器在隔离 `PREFIX` / `XDG_DATA_HOME` 下安装了 Desktop、Nemo 项和两个 Nautilus Scripts 条目。Nautilus 50.2.2 的真实 Scripts 子菜单显示两个 RustZIP 操作；在 `PATH` 不含安装目录时点击“RustZIP - Extract archives here”，RustZIP 仍从安装器创建的私有链接启动，两个 Unicode 文件的提取内容逐字节匹配源 ZIP。点击“RustZIP - Compress selected items”后 RustZIP 创建窗口成功启动；隔离安装的卸载器移除了安装文件及 Nautilus 辅助链接。
- GNOME Secret Service 的 ignored 往返测试使用唯一随机测试标签，写入、读取、列举和删除成功；Drop 清理器会在测试断言失败时尝试删除该标签。
- 在隔离配置下实际解压触发一条“RustZIP / A background task finished”通知；D-Bus 检查确认通知不含文件名或路径。新配置默认关闭通知时执行相同任务，没有发出 RustZIP 通知。

## 尚未覆盖

- Nautilus 菜单显示和点击验收运行在 Xvfb 的 Nautilus 50.2.2；真实 GNOME Wayland 会话已验证 RustZIP 启动，但未在该桌面点击菜单。Xvfb 下留有菜单、子菜单和提取后内容截图；该软件渲染环境提示 Mesa 未提供 DRI3 presentation。
- 临时 Xvfb D-Bus 会话没有 Secret Service 守护进程，RustZIP 的系统钥匙串调用在该会话中超时；GNOME Secret Service 的读写仍以真实 GNOME 会话中的独立往返测试为准。
- Windows 原生构建、Windows/macOS 语言 API、真实 IME 与屏幕阅读器，以及不同 GPU/驱动仍待设备验收。
