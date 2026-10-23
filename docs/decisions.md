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

## D4 自动更新与安装统计(已定)

- 自动更新:签名更新包(Ed25519 / minisign),GitHub Releases 加 CDN,详见 `product.md`。
- 安装统计:仅匿名 UUID 加基础环境信息,首次启动告知并可关闭,详见 `product.md`。不建用户体系。

## D5 视觉风格(已定)

采用 `docs/ui/preview.html` 的风格:冷灰底、单一青绿强调色、等宽数字、明暗主题。详见 `ui-spec.md`。

## D6 产品名称:RustZIP(已定)

- 对外显示的产品名为 `RustZIP`,大小写固定(R、Z、I、P 大写)。
- 仓库目录、crate、包名等技术标识保持小写:`rustzip`、`rz-*`。CLI 命令仍为 `rz`。
- 界面标题、安装包名、关于页、文档中的产品名一律写 `RustZIP`。

## 待决事项

- 品牌图标与 Logo。
- 官方安装包渠道(是否上架 Microsoft Store、Mac App Store、Flathub)。
- 最低支持的系统版本最终线(随 UI 框架验证结果确认)。
