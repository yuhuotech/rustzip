# 第三阶段：长尾格式候选审查与 CHM 接入

首次评估日期：2026-10-29。RustZIP 基线为 `01e84d8`。本阶段按许可证、流式读取、取消、内存 / 磁盘上限和独立样本审查 WIM、DMG、CHM、NSIS；接入满足安全门槛的 reader，其余格式明确保持不支持。2026-10-30 复查 WIM / NSIS / PDF 新候选，结论记录在本文末尾。

## 结果

- CHM 已接入只读浏览、测试和提取；reader 使用固定版本的 MIT `libchm` 源码，并补上流式范围读取、分块取消和资源上限。
- WIM 和 DMG 已加入魔数识别。RustZIP 能按内容判断格式，并明确返回“暂不支持”，不会调用未验收的解析器。
- NSIS 现只在 PE overlay 中检测到有效 FirstHeader 后识别，并给出明确的不支持读取提示；不依据 `.exe` 扩展名或普通 PE 头推断。扫描最多检查 overlay 前 8 MiB，固定读取缓冲，不按头部长度分配内存。真正浏览 / 提取仍未接入。
- 新增依赖为 MIT 的 `libchm` 0.3.0；上游源代码保存在 [`vendor/libchm`](../vendor/libchm/PATCHES.md)，来源 revision 和本地安全补丁都有记录。
- Windows / macOS 的 Open With 列表和 Linux 的桌面 / Nemo MIME 声明已加入 `.chm`；三种打包路径均携带 libchm MIT notice。清单级测试通过，实际 Windows / macOS / GNOME / Nemo 点击仍属于各平台 runner / 桌面验收。
- WIM、DMG、NSIS 仍不能浏览、测试或提取。只对这三种格式保留明确不支持状态。
- CHM 使用由独立 `rustchm` 0.1.2 编译的 fixture；WIM / DMG 签名测试使用最小字节样本，只证明分流行为，不代表这些格式可解析。

## 候选审查

| 格式 | 候选与证据 | 未满足的要求 | 阶段决定 |
|---|---|---|---|
| WIM | [`wim-parser` 0.1.2](https://crates.io/crates/wim-parser/0.1.2) 为 MIT，但只覆盖镜像元数据并对声明长度分配 XML。[`isomage` 2.1.0](https://crates.io/crates/isomage/2.1.0) 的 WIM reader 也只列镜像名，不展开镜像文件。完整候选 [`wim` 0.1.3](https://crates.io/crates/wim/0.1.3) / [`wim-format` 0.1.1](https://crates.io/crates/wim-format/0.1.1) 含 LGPL/GPL 且要求 Rust 1.96；[`ms-compress` 0.2.1](https://crates.io/crates/ms-compress/0.2.1) 含 LGPL。 | 当前完整候选涉及静态链接不允许的许可证，且高于工作区 Rust 1.93 MSRV。MIT 候选没有完整文件浏览 / 提取能力。 | 只识别 `MSWIM\0\0\0`，不注册 reader。 |
| DMG | [`dmg-core` 0.1.5](https://crates.io/crates/dmg-core/0.1.5) 为 Apache-2.0，提供 `Read + Seek` 虚拟磁盘和每块 64 MiB 输出上限，但一次缓冲由 trailer 声明的整个 XML plist，也不解析 HFS+ / APFS 文件。MIT [`isomage` 2.1.0](https://crates.io/crates/isomage/2.1.0) 只列 DMG 分区名，没有解码 `mish` 区块；MIT [`dpp` 0.6.1](https://crates.io/crates/dpp/0.6.1) 的已审查实现会对 plist、区块数或分区数据做大块分配，且缺少取消。 | 没有候选提供完整文件系统浏览 / 提取，同时满足元数据上限、流式解码与取消。 | 只识别 trailer 的 `koly`，不注册 reader。 |
| CHM | [`libchm` 0.3.0](https://crates.io/crates/libchm/0.3.0) 是 MIT、纯 Rust；上游提供 LZX 解码和结构校验，LZX 窗口最高 2 MiB，目录块最高 1 MiB。上游整条目 API 返回 `Vec`，因此 RustZIP 保留其源码并补充 64 KiB 范围读取、分块取消、4 MiB 压缩块上限、8 MiB reset-table 上限、16 MiB 目录元数据上限及 100,000 条目上限。作为对照，[`chmlib` 1.0.0](https://crates.io/crates/chmlib/1.0.0) 有范围读取，但 LGPL 且默认由 build script 静态编译；本阶段不采用。 [`rustchm` 0.1.2](https://crates.io/crates/rustchm/0.1.2) 用于独立生成测试 fixture。 | reader 满足本阶段的流式、取消和资源上限要求；独立 fixture 覆盖多块 LZX、空文件、CJK 名称和 Zip Slip。 | 接入只读 reader；写入仍不支持。 |
| NSIS | [`nsis` 0.6.0](https://github.com/ATRAPSLLC/nsis-rs/tree/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad) 为 Apache-2.0，默认限制未知长度流解压到 64 MiB，但入口要求完整 `&[u8]`，会同步把 solid 流解压成 `Vec`，没有 RustZIP 可驱动的取消或流式条目接口。 | 输入和输出仍整体缓冲；解析和固实流解压不能被调用方取消。 | 只用经验证的 PE overlay FirstHeader 做 bounded identification；reader 未通过 D37 前仍返回“不支持”，不按 `.exe` 扩展名判定。 |

## 2026-10-30 候选复查

- `wim-format` 0.1.1 和其 `wim-types` 0.1.1 声明 `LGPL-2.1-or-later OR GPL-3.0-or-later`。MIT `caddy-archive-core` 0.3.4 的可选 WIM backend 依赖 `wim-format`，故该包装不会消除底层 license 冲突；继续不接入。该底层 API 提供资源 / 分块读取原语，但没有改变项目的 license 准入条件。
- 最新 `nsis` 0.6.0 仍通过 `NsisInstallerBuilder<&[u8]>` 接收整个输入；solid payload 会先解到受预算限制的缓存 `Vec`。预算限制缓解输出膨胀，但输入整包缓冲和缺少调用方取消的边界仍不符合本项目 reader 准入要求。
- `pdf-engine` 1.0.0 提供单 stream / 单文档内存 / 图像像素上限，但 crate license 是 `AGPL-3.0-only OR LicenseRef-PDFluent-Commercial`；这两种选择都不在当前依赖许可策略内。PDF 仍交给系统阅读器。

## 2026-10-30 当前上游源码核验

- `nsis-rs` 当前主分支 Cargo.toml 声明 0.6.0；上游 0.3.0 起已把未知长度数据的默认解压预算设为 64 MiB，并允许调用方调小预算。但入口仍是 `NsisInstallerBuilder<&[u8]>`，固实数据解码结果保存在 `Vec<u8>`，没有可中断的 `Read` / 回调接口。该更新只补强膨胀上限，仍不满足 D37 的流式输入与分块取消条件。依据：[0.3.0 解码预算变更](https://github.com/ATRAPSLLC/nsis-rs/releases)、[当前 parser 实现](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/src/installer/nsisinstaller.rs)。
- `dpp` 0.6.1 的 MIT pipeline 已默认将分区数据流入匿名临时文件，并提供文件到 writer 的流式 API；但底层 `udif` 打开时仍按 trailer 声明长度把 plist 和整个 data fork 读入 `Vec`，分区 block 解码也按 block map 声明大小分配缓冲区。`DmgReaderOptions` 只有校验和选项，没有元数据 / block / partition 限额或取消 token；匿名临时分区没有磁盘配额，HFS+ / APFS 遍历会收集 `Vec` 且无调用方取消。故新版改善了通常路径的峰值内存，但仍不满足 D37，DMG 暂不接入。依据：[固定上游提交](https://github.com/Dil4rd/dpp/tree/3a73ee28fdab192fcdfbe8edcad7b520867f2417)、[UDIF reader](https://github.com/Dil4rd/dpp/blob/3a73ee28fdab192fcdfbe8edcad7b520867f2417/udif/src/reader.rs)、[pipeline](https://github.com/Dil4rd/dpp/blob/3a73ee28fdab192fcdfbe8edcad7b520867f2417/dpp/src/pipeline.rs)、[APFS reader](https://github.com/Dil4rd/dpp/blob/3a73ee28fdab192fcdfbe8edcad7b520867f2417/apfs/src/lib.rs)。
- D47 增加 NSIS 的受限内容识别：PE section table 和 FirstHeader 都需结构有效，扫描 overlay 最多 8 MiB，不使用文件声明长度分配；普通 PE 仍为未知格式。全工作区 all-features 测试和 Clippy 通过；本机 nightly ASan 对合成 PE 种子运行 119,304 次无崩溃，GitHub run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 又运行 3,287,482 次并保存 corpus cache。另用上游实际 NSIS fixture `deflate_nonsolid.exe` 验证 `rz l` 给出专属“不支持读取”错误。该识别器不是 NSIS reader。

### 2026-10-30 补充候选复查

- [`dmg-core` 0.1.5](https://docs.rs/dmg-core/0.1.5/dmg/) 是纯 Rust、提供 `Read + Seek` 的 UDIF 虚拟磁盘 reader；但 `DmgReader::open` 将 trailer 中的 `xml_length` 直接用于分配 plist 缓冲，虽检查区域未越过文件末尾，却没有元数据硬上限或取消接口。压缩块输出有 64 MiB 上限，但仍会按 run 输入长度分配压缩缓冲；它本身也不遍历 HFS+ / APFS 文件系统。故此候选仍不满足 D37。[源码](https://docs.rs/dmg-core/0.1.5/src/dmg/lib.rs.html)
- [`nsis-rs`](https://github.com/ATRAPSLLC/nsis-rs) 现有 Apache-2.0 许可并覆盖 NSIS 1.x / 2.x / 3.x；其公开入口仍从整份 `&[u8]` 构造 installer，提取文件通过内存数据和解压 `Vec` 暴露，未提供 RustZIP 可驱动的分块取消与流式条目 reader，故不能注册为后端。[上游 API 与文件提取示例](https://github.com/ATRAPSLLC/nsis-rs#quick-start)
- [`newtua-formats`](https://github.com/new-the-unarchiver/newtua-formats) 新增了纯 Rust NSIS decoder，但整个项目声明 LGPL-3.0-or-later；同时其通用示例先把输入 `read` 成完整 `Vec`，NSIS 2.0+ / 3.x 范围也不覆盖全部既有版本。许可和读取边界均不满足本项目条件。
- WIM 复查未发现同时具备宽松许可、完整镜像文件遍历、硬资源上限和调用方取消的 reader。既有 `wim-format` / `caddy-archive-core` 依赖关系仍受 LGPL / GPL 许可限制，继续只识别并明确提示不支持。

候选源码核对点：[`dmg-core` trailer 与 XML plist 缓冲](https://docs.rs/crate/dmg-core/0.1.5/source/src/lib.rs)、[`isomage` WIM 元数据范围](https://github.com/JackDanger/isomage/tree/v2.1.0/src/formats)、[`libchm` 上游源码](https://github.com/trypsynth/libchm/tree/927cec11c4cf8165d28731fe1b3e07b40f8ddf5e)、[RustZIP 的局部补丁和上游来源](../vendor/libchm/PATCHES.md)。NSIS [`from_bytes 与解压上限`](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/src/installer/nsisinstaller.rs#L305-L345)、[`固实流预解压`](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/src/installer/nsisinstaller.rs#L607-L660)。

## 实现与验证

- `rz-formats::sniff` 依据 WIM / CHM 头部、DMG 512-byte trailer 及 NSIS 的 PE overlay FirstHeader 识别格式；文件扩展名不参与判断。NSIS reader 仍不支持。
- `rz-formats::open` 对 WIM / DMG 返回明确的 `Error::Unsupported`；CHM 返回只读 `ArchiveReader`。
- `chm_basic.chm` 由独立 `rustchm` 编译器生成，覆盖多块 LZX、空文件、CJK 名称和误导性 `.bin` 扩展名。定向测试覆盖列举 / 内容逐字节比对、输出上限、元数据取消、多块解码中的取消、损坏输入和 Zip Slip 拒绝。
- `cargo test` 与 `cargo test --all-features` 全工作区通过；各有 3 项按项目定义忽略的环境测试（两个要求非 root Unix 用户、一个真实系统钥匙串）。`cargo clippy --all-targets -- -D warnings`、工作区 / vendor 格式检查、vendor `libchm` 50 项单元测试 + 2 项文档测试及独立 Clippy 均通过。现有依赖 `block v0.1.6` 仍有 Rust future-incompatibility 提示。
- `cargo check --manifest-path fuzz/Cargo.toml --bins` 和 `cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings` 通过，新增 CHM fuzz target。2026-10-29 本机 nightly ASan fuzz 对独立 CHM fixture 运行 15 秒，约 21,000 次变异未发现崩溃；之后 GitHub Fuzz run [40961694769](https://github.com/yuhuotech/rustzip/actions/runs/40961694769) 对 CHM 运行 34,172 次并保存 corpus cache，未发现崩溃。此前稳定工具链确实无法运行 sanitizer session。本机也对有效 bzip2 seed 执行了 15 秒 ASan fuzz，约 11,000 次变异未发现崩溃。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 通过现有格式、转换、修复与卷操作的输出校验。该基准不包含 CHM，结果只用于确认原有基准路径正常，不作为 CHM 性能数据。
- 本阶段未运行 Windows / Linux CI runner；平台关联配置有清单级测试，真实安装器及桌面点击仍待相应 runner 实测。
- [第三阶段后端门槛](decisions.md#d37-长尾格式后端准入) 记录了后续 reader 必须达到的安全条件。

## 后续准入条件

WIM、DMG、NSIS 后续 reader 仍必须提供：流式 `Read` / 范围读取；分块取消和进度；对元数据、解压内存、总输出及临时磁盘的硬上限；路径规范化和特殊文件策略；损坏输入测试、fuzz 目标及实际格式工具生成的独立 fixture；三平台构建与互通验收。任一条件缺失时，继续保持“不支持”。

## 2026-10-31 PDF 安全方案更新

- 依据 D50，接入 Apache-2.0 OR MIT 的 Hayro 0.7.1，但只放在独立 `rz-pdf-renderer` 子进程中。全局分配器限制同时存活 heap 请求与累计分配，helper 自带 10 秒 watchdog；GUI 父进程还能取消或超时终止 helper。
- PDF 内容限制为 16 MiB，首屏栅格不超过 16,777,216 像素 / 单边 8,192 像素，PNG 不超过 8 MiB。测试覆盖正常渲染、超大页面缩放、300 MiB Flate 解压炸弹和未关闭输入管道时 helper 自行退出。
- 安装包需要把 helper 与桌面程序放在同一目录；三平台构建脚本与相关许可证已更新。当前本机安全测试和 Linux 安装脚本冒烟通过，macOS app 包、Windows 安装器及三平台运行时验收仍待完成。
