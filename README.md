# RustZIP

用 Rust 开发的跨平台压缩管理软件(Windows / macOS / Linux),追求极致性能和简洁美观的体验。

项目文档见 [`docs/`](docs/):产品需求、技术架构、技术规范、UI 规范、决策记录。AI 协作规则见 [`AGENTS.md`](AGENTS.md)。

## 当前进度

| 能力 | 状态 |
|---|---|
| zip 读写(含 AES-256 加密) | 已完成 |
| 7z 读写(固实块、LZMA2 多线程、AES-256 加密、文件名加密) | 已完成 |
| tar 及 tar.gz / tar.xz / tar.zst / tar.bz2 读写 | 已完成 |
| 单文件格式 .gz / .xz / .zst / .bz2 读写 | 已完成 |
| rar 只读(可选 feature `rar`,默认关闭) | 已完成,见下方说明 |
| 安全解压(路径穿越、压缩炸弹、符号链接、setuid) | 已完成 |
| 取消、进度回调、原子写入(失败不留半成品) | 已完成 |
| 权限与修改时间保留 | 已完成 |
| zip 多线程压缩(保持条目顺序、内存预算) | 已完成 |
| zip 多线程解压 | 已完成 |
| xz / zstd / 7z(LZMA2)内部多线程压缩 | 已完成 |
| 命令行 `rz a / l / x / t`(含 `-e` 指定文件名编码) | 已完成 |
| zip 中日韩文件名编码自动识别(GBK / Shift-JIS / EUC-KR 等)与手动指定 | 已完成 |
| 任务队列:并发上限、暂停 / 继续 / 取消、速度与剩余时间、事件订阅、密码失败后重试 | 已完成(`rz-app`) |
| 设置:原子保存、损坏自动隔离、范围校验、各系统配置目录 | 已完成(`rz-app`) |
| 智能解压(多个顶层项时自动建同名文件夹,不混入已有文件夹) | 已完成(`rz-app`) |
| 更新校验:Ed25519 签名绑定版本 / 平台 / 哈希,防降级与换包 | 已完成,界面已接入(状态栏提示新版本、下载、验签、交给系统安装);更新源为 GitHub Release |
| 匿名安装统计:激活 + 每日心跳,可关闭,失败不重试风暴 | 已完成,HTTP 已接入;**需要自建接收服务并设置 `RZ_STATS_URL`**,见 `docs/release.md` |
| 密码本 | 已完成，侧栏支持新增、编辑、删除；系统钥匙串存储，设置里"记住密码"默认关闭，开启后自动使用同名压缩包的保存密码；核心另提供候选密码查找 |
| 常用位置 | 系统默认目录 + 自动记录已打开压缩包所在目录；单击选中、双击进入、右键移除记录；列表/小图标切换，进入文件夹后沿用，重启保留 |
| 发布工具:生成签名密钥、给安装包签名、生成 `update.json` | 已完成(`rz-release`) |
| 应用图标、macOS 打包(.app / .dmg)、Windows 安装程序脚本、Linux 压缩包 | 图标和 macOS 打包已在本机跑通;Windows / Linux 打包和发布工作流**未实际运行过** |
| 文件关联与右键菜单 | macOS 仅"打开方式";Windows 注册表和 Linux(Dolphin / Nemo)菜单已写好,未实机验证;Finder 右键与 Quick Look 未做 |
| 7z 分块并行解压、bzip2 多线程、大文件分块 deflate | 未开始 |
| 压缩包内添加 / 删除(zip、7z、tar 系列;解包到旁边、改完重新打包、校验后整体替换,失败原包不动) | 已完成(`rz-app::modify`) |
| 预览(图片在详情栏直接显示;其他文件解到临时目录用系统程序打开,程序和脚本不打开) | 已完成,重命名和原地增量修改未做 |
| 格式转换 | 已完成，流式转换为 ZIP / 7z / tar 系七种目标格式，独立源/目标密码，校验后另存，不覆盖源包；符号链接暂不支持 |
| ZIP / 7z 修复 | 已完成，尽力抢救可读条目并报告跳过项；ZIP 支持丢失中央目录恢复，7z 继续尝试后续独立块；严重损坏不保证恢复 |
| 分卷 | 未开始 |
| 选择性解压(按文件或文件夹,各格式含固实块与并行路径) | 已完成 |
| 压缩包浏览模型(目录树、自然排序、搜索、面包屑、体积统计) | 已完成(`rz-app`) |
| 桌面应用 `rustzip`(GPUI Kit):标签页、工具栏、文件表格、详情栏、任务面板、状态栏、新建压缩面板、密码提示、拖拽、快捷键、菜单、深浅色跟随系统 | macOS 无头界面测试与真实 Metal 离屏渲染验证，Windows / Linux 未验证；侧栏工具的范围和记录见 `docs/sidebar-implementation.md` |

### 关于 RAR

RAR 只读,不实现压缩。解压依赖 unRAR 源码(`unrar` crate),它的许可证禁止用来重建 RAR 压缩算法,因此放在默认关闭的 feature 里:

```
cargo build --release --features rar
```

unRAR 是 C++ 代码,处理的是不可信输入;重要场景下建议在受限环境中使用。

## 命令行

```
rz a <archive> <path>... [-l 0-9] [-p password] [-t threads]   创建
rz l <archive> [-p password]                                   列出内容
rz x <archive> [-o dir] [-p password] [-t threads] [-y|-n]     解压(默认同名保留两者并加序号,-y 覆盖,-n 跳过已有)
rz t <archive> [-p password]                                   校验完整性
```

创建格式由扩展名决定:`.zip .7z .tar .tar.gz .tar.xz .tar.zst .tar.bz2`(及 `.tgz .txz .tzst .tbz2`),
单文件 `.gz .xz .zst .bz2`。读取时按文件头识别,不看扩展名。

## 桌面应用

```
cargo run --release -p rustzip-gui            # 或:cargo run --release -p rustzip-gui -- 某个压缩包.zip
```

快捷键:`⌘O` 打开、`⌘N` 新建压缩、`⌘F` 搜索、`⌘E` 解压全部、`⌘W` 关闭标签页(Windows / Linux 为 Ctrl)。
侧栏工具：`⌘⇧B` 常用位置、`⌘⇧P` 密码本、`⌘⇧C` 转换格式、`⌘⇧R` 修复压缩包；也可从“工具”菜单进入。
也可以把压缩包拖进窗口打开,把普通文件拖进窗口压缩。设置保存在系统配置目录的 `settings.json`。

不打开窗口也能看到界面长什么样:用真实的 Metal 渲染器离屏截图(浅色和深色各 6 个状态):

```
RZ_SHOT_DIR=/某个文件夹 cargo run --release -p rustzip-gui --example screenshots
```

## 开发

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p rz-engine
```

许可证:`MIT OR Apache-2.0`(启用 `rar` feature 的构建还需遵守 unRAR 的许可条款)。
