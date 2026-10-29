# 第三阶段：长尾格式候选审查与 CHM 接入

评估日期：2026-10-29。RustZIP 基线为 `01e84d8`。本阶段按许可证、流式读取、取消、内存 / 磁盘上限和独立样本审查 WIM、DMG、CHM、NSIS；接入满足安全门槛的 reader，其余格式明确保持不支持。

## 结果

- CHM 已接入只读浏览、测试和提取；reader 使用固定版本的 MIT `libchm` 源码，并补上流式范围读取、分块取消和资源上限。
- WIM 和 DMG 已加入魔数识别。RustZIP 能按内容判断格式，并明确返回“暂不支持”，不会调用未验收的解析器。
- NSIS 暂不从 `.exe` 扩展名或普通 PE 头推断。NSIS 数据位于 PE overlay，需要验证内部 FirstHeader；普通 Windows 可执行文件不能一概识别成归档。
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
| NSIS | [`nsis` 0.6.0](https://github.com/ATRAPSLLC/nsis-rs/tree/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad) 为 Apache-2.0，默认限制未知长度流解压到 64 MiB，但入口要求完整 `&[u8]`，会同步把 solid 流解压成 `Vec`，没有 RustZIP 可驱动的取消或流式条目接口。 | 输入和输出仍整体缓冲；解析和固实流解压不能被调用方取消。扩展名 `.exe` 不能区分 NSIS 与其他 Windows 可执行文件。 | 不新增 PE / NSIS 启发式识别，不注册 reader。 |

候选源码核对点：[`dmg-core` trailer 与 XML plist 缓冲](https://docs.rs/crate/dmg-core/0.1.5/source/src/lib.rs)、[`isomage` WIM 元数据范围](https://github.com/JackDanger/isomage/tree/v2.1.0/src/formats)、[`libchm` 上游源码](https://github.com/trypsynth/libchm/tree/927cec11c4cf8165d28731fe1b3e07b40f8ddf5e)、[RustZIP 的局部补丁和上游来源](../vendor/libchm/PATCHES.md)。NSIS [`from_bytes 与解压上限`](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/src/installer/nsisinstaller.rs#L305-L345)、[`固实流预解压`](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/src/installer/nsisinstaller.rs#L607-L660)。

## 实现与验证

- `rz-formats::sniff` 依据 WIM / CHM 头部和 DMG 512-byte trailer 识别格式；文件扩展名不参与判断。
- `rz-formats::open` 对 WIM / DMG 返回明确的 `Error::Unsupported`；CHM 返回只读 `ArchiveReader`。
- `chm_basic.chm` 由独立 `rustchm` 编译器生成，覆盖多块 LZX、空文件、CJK 名称和误导性 `.bin` 扩展名。定向测试覆盖列举 / 内容逐字节比对、输出上限、元数据取消、多块解码中的取消、损坏输入和 Zip Slip 拒绝。
- `cargo test` 与 `cargo test --all-features` 全工作区通过；各有 3 项按项目定义忽略的环境测试（两个要求非 root Unix 用户、一个真实系统钥匙串）。`cargo clippy --all-targets -- -D warnings`、工作区 / vendor 格式检查、vendor `libchm` 50 项单元测试 + 2 项文档测试及独立 Clippy 均通过。现有依赖 `block v0.1.6` 仍有 Rust future-incompatibility 提示。
- `cargo check --manifest-path fuzz/Cargo.toml --bins` 和 `cargo clippy --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings` 通过，新增 CHM fuzz target。2026-10-29 本机 nightly ASan fuzz 对独立 CHM fixture 运行 15 秒，约 21,000 次变异未发现崩溃；此前稳定工具链确实无法运行 sanitizer session。短时 fuzz 不替代持续 runner。本机也对有效 bzip2 seed 执行了 15 秒 ASan fuzz，约 11,000 次变异未发现崩溃。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 通过现有格式、转换、修复与卷操作的输出校验。该基准不包含 CHM，结果只用于确认原有基准路径正常，不作为 CHM 性能数据。
- 本阶段未运行 Windows / Linux CI runner；平台关联配置有清单级测试，真实安装器及桌面点击仍待相应 runner 实测。
- [第三阶段后端门槛](decisions.md#d37-长尾格式后端准入) 记录了后续 reader 必须达到的安全条件。

## 后续准入条件

WIM、DMG、NSIS 后续 reader 仍必须提供：流式 `Read` / 范围读取；分块取消和进度；对元数据、解压内存、总输出及临时磁盘的硬上限；路径规范化和特殊文件策略；损坏输入测试、fuzz 目标及实际格式工具生成的独立 fixture；三平台构建与互通验收。任一条件缺失时，继续保持“不支持”。
