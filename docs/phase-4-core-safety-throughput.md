# 第四阶段：解压安全、并行编解码与依赖修复

开始日期：2026-10-29。基线提交：`8718b05`。本阶段从路线图和平台验收记录继续，保留尚未满足安全条件的格式为“不支持”。

## 已完成

- **RAR 输出限制**：UnRAR 的逐块数据回调封装在 `rz-ffi`。选中条目按声明大小、压缩比和剩余 `max_total` 限量写入单条目临时文件；未选条目测试解码但不落临时盘；解码中可取消。整条目 `ArchiveReader::open` 读取限制为 64 MiB。启用可选 `rar` feature 时恢复受限图片 / 文本预览，默认包仍关闭 RAR。见决策 D38。
- **bzip2 并行解压**：通过 MIT `pbz2` 0.2.0 解码 bzip2 单流和串接 member，按配置线程数和内存预算估算 worker，预算不足时回退 `MultiBzDecoder`。本地 vendor 补丁让 worker / scanner 启动故障返回错误；随包携带 MIT license 与 NOTICE。
- **gzip 分块 Deflate**：`.gz` / `.tar.gz` 多线程压缩按最多 900,000 字节分块，生成按顺序连接的标准 gzip members；低预算时回退 flate2。
- **ZIP 大条目分块 Deflate**：未加密 Deflate 条目按 900,000 字节分块并行压缩，按输入顺序写入有界匿名临时 ZIP，再由 `zip` 9.1.3 合并压缩数据。worker 数与每 worker 两个待处理块按 16 MiB / worker 估算预算；Store、加密及单 worker 情况沿用顺序 writer。见决策 D41。
- **ZIP 依赖升级**：工作区统一升级到 `zip` 9.1.3，替换受安全公告影响的 2.6.1 与旧 zstd 依赖。应用解压继续走 RustZIP 引擎安全路径。cargo-deny 配置见仓库根目录 `deny.toml`；无安全升级的上游维护状态公告保留有原因的忽略项。
- **ZIP 编辑元数据保真**：快路径放开归档 / 条目注释输入，复制时保留归档注释、ZIP64 扩展数据及未修改条目的文件注释 / 扩展字段；拒绝带自解压前缀的快路径输入。通过删除、重命名后逐字节检查注释和自定义 extra field。见 D42。
- **fuzz**：新增 bzip2、RAR fuzz target 和 bzip2 seed。nightly ASan 对 bzip2、CHM、RAR、ZIP recovery 分别运行约 15 秒，约 11,000、21,000、67,000、91,000 次变异，未发现崩溃。短时结果不替代持续 fuzz runner。
- **持续 fuzz runner**：新增 `.github/workflows/fuzz.yml`，每日运行四个 nightly ASan target，最多同时跑两个；手动运行时每 target 60 秒，定时运行每 target 180 秒；保留 14 天崩溃 artifact，并按 target 缓存 corpus。`fuzz/seeds/` 保存最小 ZIP、bzip2、CHM 和 RAR 输入。工作流尚未在 GitHub 上运行；首次远端运行与后续覆盖增长仍待验收。

## 本机验证

- `cargo fmt --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；仍有第三方 `block v0.1.6` future-incompatibility 提示。
- `cargo test --workspace --all-features` 通过；2 项需非 root Unix 环境、1 项真实系统钥匙串用例按项目设置忽略。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 通过，并验证基准输出。单次 8 MiB 结果不作通用性能结论。
- ZIP 9.1.3 升级后，`RZ_BENCH_MB=8 cargo bench -p rz-app --bench zip_edit` 的 raw-copy 与 repack 两路均按文件哈希校验通过；单次 8 MiB 结果不作通用性能结论。
- `cargo fuzz run zip_recovery` 在 ZIP 9.1.3 上进行约 91,000 次短时变异，未发现崩溃。
- `cargo deny check --hide-inclusion-graph` 通过 licenses、sources、bans、advisories；仍报告多个传递依赖版本并存。advisory 忽略项都来自上游注明暂无安全升级的依赖。
- 上述依赖审计记录来自本阶段较早的检查。最终改动只将已有传递依赖 `crc32fast` 加为直接依赖；本次环境没有安装 `cargo-deny` 可执行文件，未能重跑依赖审计。该版本的 Cargo.toml 声明 `MIT OR Apache-2.0`，符合项目许可白名单。
- 本机 nightly ASan 下四个 fuzz target 均构建成功；reviewed seed 各运行 5 秒，ZIP recovery / CHM / bzip2 / RAR 分别约 29,605 / 7,995 / 4,462 / 20,552 次变异，未发现崩溃。`actionlint` 检查 `.github/workflows/fuzz.yml` 通过；Actions 远端每日任务仍待首次执行。
- `cargo test -p rz-formats --test zip_parallel --all-features` 通过 6 项；36 MiB CJK 名称、重复块伪随机内容分块往返通过本机 `unzip -t`。引擎 40 MiB 文件流式解压和取消清理用例通过；源在第二块后失败时 worker 正常退出。
- `cargo test -p rz-app --lib zip_copy_edit_preserves_archive_and_entry_comments_and_extra_fields` 通过；验证删除和重命名后的归档注释、文件注释及自定义 extra field。
- 本机 nightly ASan 下四个 fuzz target 均构建成功；用 reviewed seed 各运行 5 秒，ZIP recovery / CHM / bzip2 / RAR 分别约 29,605 / 7,995 / 4,462 / 20,552 次变异，未发现崩溃。`actionlint` 检查 `.github/workflows/fuzz.yml` 通过。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 全矩阵完成并校验输出；ZIP 单条目基准以 BLAKE3 校验 40 / 64 / 128 MiB 样本，每尺寸每设置 3 轮。详细环境和中位数见[基准记录](benchmarks/2026-10-29-zip-large-deflate.md)。

## 后续仍需完成

- ZIP 9.1.3 的 8 MiB 编辑对照基准已运行；仍需多尺寸、多次采样。最新代码尚未跑 Windows / macOS / Linux CI，真实安装包和系统交互验收也待完成。
- ZIP 大条目分块 Deflate 已接入；本机 40 / 64 / 128 MiB × 3 轮基准通过内容校验，仍需 Windows / macOS 外部工具矩阵和其他文件类型性能样本。合格 ZIP 编辑快路径的注释和扩展字段保真见 D42；真正原地增量编辑仍需先处理 D11 的崩溃保全边界。
- 持续 fuzz 工作流与种子已加入；需在仓库 Actions 上启动首次每日运行，检查所有四个 target 结果、缓存语料增量及崩溃 artifact 保存路径。
- WIM、DMG、NSIS 的安全只读能力；当前只有 WIM / DMG 识别，NSIS 不推测识别。
- PDF / 音频 / 视频应用内预览，关机等高影响完成后动作。
- 压缩包对比、虚拟挂载、云端按需解压、WASM 插件和 SFX 仍为远期需求。
