# Ubuntu 26.04 Linux 验收记录

日期：2026-10-27。目标环境：用户提供的 Ubuntu 26.04.1 LTS、x86_64、GNOME Wayland。验证工作区和输出均位于专用 `/tmp/rustzip-linux-e2e-20261026/` 与 `/var/tmp/rustzip-linux-e2e-target/`。

## 核心和 GUI

- Rust 1.96.0；`cargo fmt --all -- --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `xvfb-run -a env LIBGL_ALWAYS_SOFTWARE=1 cargo test --workspace --all-features --quiet` 通过；GUI 58 项测试全部通过，语言测试覆盖十种菜单选择、357 个资源键、翻译占位符和侧栏固定宽度。全工作区有 3 项环境条件测试按定义忽略。
- `cargo build --all-features -p rz -p rustzip-gui` 和 `cargo build --release -p rustzip-gui` 通过。RustZIP 发布 tarball 成功生成。
- GUI 在真实 GNOME Wayland 会话中启动并保持运行 18 秒；另在 Xvfb 软件渲染环境启动并保持运行 12 秒。Xvfb 输出 Mesa 未提供 DRI3 presentation 的提示；真实 Wayland 启动无错误日志。
- 远端系统 locale 为 `LANG=en_US.UTF-8`。本机 macOS 首选语言为 `zh-Hans-CN`；`sys-locale` 返回的首选标签按主语言码映射到 RustZIP 的简体中文选项。

## 归档与系统集成

- RustZIP 创建的分卷 ZIP 和 7z 均由 p7zip 26.00 测试并解压，内容与源目录逐文件比对一致。
- Info-ZIP 3.0 创建的 split ZIP 和 p7zip 创建的分卷 7z 均由 RustZIP 测试、解压并逐文件比对一致；样本包含中日韩文件名和随机数据。
- Linux tarball 的免 root 安装器在隔离 `PREFIX` / `XDG_DATA_HOME` 下安装了 Desktop、Nemo 项和两个 Nautilus Scripts 条目。Nautilus 解压脚本通过 Xvfb 启动 RustZIP 并完成真实提取，结果与源目录一致；压缩脚本启动 RustZIP 的创建窗口；卸载器移除了隔离安装中的文件。

## 尚未覆盖

- RustZIP 已在真实 GNOME Wayland 会话启动；当前没有可用的截图/指针控制通道，因此未点击 Nautilus 窗口确认 Scripts 菜单的显示。脚本目录、执行行为与 Xvfb 下的完整提取流程已验证；GNOME Shell 拒绝截图 D-Bus 请求，所以没有保存真实桌面截图。
- Windows 原生构建、Windows/macOS 语言 API、真实 IME 与屏幕阅读器，以及不同 GPU/驱动仍待设备验收。
