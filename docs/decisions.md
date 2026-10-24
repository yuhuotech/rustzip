# 决策记录

记录已做的重要决策及理由。要推翻其中任何一条,先在这里新增一条并说明原因。

## D1 项目许可证:`MIT OR Apache-2.0`(已定)

- 选宽松许可证,因为它能直接使用的库最多、分发限制最少:Rust 压缩类 crate、libarchive(BSD)、zstd、lz4、BLAKE3 都可直接使用。
- 不选 GPL:会挡住 macOS App Store 等分发渠道,与 unRAR 的授权混合有争议;Apache-2.0 自带专利授权,对压缩算法这类领域更安全。
- 不拷贝 7-Zip 源码(LGPL,静态链接不适合),按格式规范重写。
- RAR 只读,做成独立 Cargo feature `rar`,官方构建附带 unRAR 授权声明,发行版可关闭。

## D2 RAR 只读(已定)

只做解压、浏览、测试,不做 RAR 压缩。

## D3 UI 框架:首选 GPUI Kit,待验证(暂定)

- 不选 Tauri:依赖系统 WebView,Linux 的 WebKitGTK 在渲染、性能、版本上不一致,与"极致性能和兼容性"冲突;内存和冷启动也偏高。
- 不选 Slint 作为首选:授权需走 Royalty-Free 并署名,或整体 GPLv3,与项目许可证有摩擦。保留为**备选**。
- 首选 **GPUI Kit**(`longbridge/gpui-kit`):Apache-2.0、纯 Rust、GPU 渲染、自带虚拟化表格、Dock、AccessKit。
- 风险:版本为 0.x、依赖固定的 GPUI 快照;官网和 README 未提中文输入法;Linux 与老显卡兼容性无数据;无 GPU 时能否回退 CPU 渲染不明。
- **M0 必须通过三项验证才能定案**,任何一项失败则退回 Slint:
  1. 三平台文本框中文输入(拼音、五笔、日文),含密码框和快捷键。
  2. 渲染 100 万行条目的压缩包列表,测滚动帧率和内存。
  3. Linux 虚拟机、无独显旧机器、Windows 10 上启动和渲染无花屏。
- 验证未完成前,UI 代码必须与 `rz-app` 解耦,保证能换框架。
- 进展:`gpui-kit 0.8.0` 已在 macOS(Apple Silicon,Xcode Metal)上编译通过,release 构建约 15 MB;验证程序 `apps/gpui-probe` 能启动并持续运行 8 秒无崩溃。清单和判定标准见 `gpui-probe.md`。
- 用户在 macOS 上的实测(未说明具体机型和版本):两个输入框正常;100 万行表格滚动稳定在 120 帧、非常顺滑。发现一个问题:**深色模式不会自动跟随系统**。原因是 GPUI Kit 不自动同步主题,应用必须在窗口创建时调用 `Theme::sync_system_appearance`,并用 `window.observe_window_appearance` 在系统切换时再调用一次。验证程序已按此修复,但修复后的效果还没有人在真机上确认。正式 GUI 必须照做。
- 仍未验证:Windows、Linux 上的输入法和渲染,以及无独显 / 虚拟机环境。这几项不通过就不能定案。

## D4 自动更新与安装统计(已定)

- 自动更新:签名更新包(Ed25519 / minisign),GitHub Releases 加 CDN,详见 `product.md`。
- 安装统计:仅匿名 UUID 加基础环境信息,首次启动告知并可关闭,详见 `product.md`。不建用户体系。

## D5 视觉风格(已定)

采用 `docs/ui/preview.html` 的风格:冷灰底、单一青绿强调色、等宽数字、明暗主题。详见 `ui-spec.md`。

## D6 产品名称:RustZIP(已定)

- 对外显示的产品名为 `RustZIP`,大小写固定(R、Z、I、P 大写)。
- 仓库目录、crate、包名等技术标识保持小写:`rustzip`、`rz-*`。CLI 命令仍为 `rz`。
- 界面标题、安装包名、关于页、文档中的产品名一律写 `RustZIP`。

## D7 M1 的格式与并行实现选择(已定)

- 7z:`sevenz-rust2`(Apache-2.0,纯 Rust)。写入走固实块加多线程 LZMA2,加密时同时加密文件头。
- xz:`liblzma`(静态链接 xz 源码,公有领域),用其多线程编码器。zstd 用 `zstd` 的多线程模式。
- bzip2:`bzip2` crate,目前单线程。
- 读取接口加入 `for_each`(单遍流式)和 `reopen`(独立读取器):tar / 7z / rar 这类顺序格式一次解码全部条目,zip 用多个读取器并行解压。
- zip 并行压缩:小文件在工作线程里各自压成内存中的小 zip,再由写入线程按输入顺序合并(`merge_archive`),受内存预算约束;超过阈值的大文件在写入线程内联压缩。输出与单线程一致,条目顺序不受线程数影响。
- 压缩炸弹:压缩比限制只对解压后超过 256 MiB 的条目生效,避免误伤小而高度重复的数据;总大小上限和"写入不超过声明大小"始终生效。
- RAR:`unrar` crate,放在默认关闭的 `rar` feature 里,只读。unRAR 是 C++ 代码,攻击面比其余纯 Rust 后端大,需要在文档中提示。
- 已知局限:每个压缩比检查依赖条目声明的压缩后大小,7z / tar 这类固实格式没有逐条目的压缩后大小,只能靠总大小上限和"不超过声明大小"兜底。

## D8 联网、钥匙串与发布(已定)

- 联网与系统钥匙串放在独立 crate `rz-services`(HTTP 用 `ureq` + rustls,钥匙串用 `keyring`),`rz-app` 只定义 trait 并用假实现测试;GUI 通过它接入,核心 crate 不碰网络。
- 更新源:GitHub Release 里的 `update.json`(`releases/latest/download/update.json`),安装包也放在同一个 Release。信任来自内置 Ed25519 公钥,不来自 GitHub 或 TLS;私钥只存在维护者本机和仓库 Secret `RZ_UPDATE_KEY`。
- 更新只做到"下载、验签、交给系统打开安装",不静默替换运行中的程序。
- 统计:只有设置了 `RZ_STATS_URL`(构建期或运行期)才会上报,且必须在用户看过首次启动提示之后。服务端自建,GitHub 不能接收上报。
- 记住密码默认关闭,开启后存进系统钥匙串,条目名只用压缩包文件名。
- 文件关联用"备选"优先级,不抢用户已选的默认程序。
- 品牌图标:拉链图案、青绿渐变圆角方块,由代码绘制(`make_icons` 示例),不依赖设计软件。

## 待决事项

- 官方安装包渠道(是否上架 Microsoft Store、Mac App Store、Flathub)。
- 最低支持的系统版本最终线(随 UI 框架验证结果确认)。
