# 第四阶段：解压安全、并行编解码与依赖修复

开始日期：2026-10-29。基线提交：`8718b05`。本阶段从路线图和平台验收记录继续，保留尚未满足安全条件的格式为“不支持”。

## 已完成

- **RAR 输出限制**：UnRAR 的逐块数据回调封装在 `rz-ffi`。通用选择性提取的选中条目按声明大小、压缩比和剩余 `max_total` 限量写入单条目临时文件；未选条目测试解码但不落临时盘；解码中可取消。整条目 `ArchiveReader::open` 读取限制为 64 MiB，单条目通过有界内存缓冲读取，不写临时盘；GUI 预览另有 8 MiB 图片 / 64 KiB 文本限制。默认包仍关闭 RAR。见决策 D38。
- **bzip2 并行解压**：通过 MIT `pbz2` 0.2.0 解码 bzip2 单流和串接 member，按配置线程数和内存预算估算 worker，预算不足时回退 `MultiBzDecoder`。本地 vendor 补丁让 worker / scanner 启动故障返回错误；随包携带 MIT license 与 NOTICE。
- **gzip 分块 Deflate**：`.gz` / `.tar.gz` 多线程压缩按最多 900,000 字节分块，生成按顺序连接的标准 gzip members；低预算时回退 flate2。
- **ZIP 大条目分块 Deflate**：未加密 Deflate 条目按 900,000 字节分块并行压缩，按输入顺序写入有界匿名临时 ZIP，再由 `zip` 9.1.3 合并压缩数据。worker 数与每 worker 两个待处理块按 16 MiB / worker 估算预算；Store、加密及单 worker 情况沿用顺序 writer。见决策 D41。
- **ZIP 依赖升级**：工作区统一升级到 `zip` 9.1.3，替换受安全公告影响的 2.6.1 与旧 zstd 依赖。应用解压继续走 RustZIP 引擎安全路径。cargo-deny 配置见仓库根目录 `deny.toml`；无安全升级的上游维护状态公告保留有原因的忽略项。
- **ZIP 编辑元数据保真**：快路径放开归档 / 条目注释输入，复制时保留归档注释、ZIP64 扩展数据及未修改条目的文件注释 / 扩展字段；拒绝带自解压前缀的快路径输入。通过删除、重命名后逐字节检查注释和自定义 extra field。见 D42。
- **ZIP 跨平台权限**：便携模式明确写入 Unix creator 标记和目录 `0755` / 文件 `0644`，不再依赖运行平台的 ZIP 默认值；并行大条目 Deflate 使用相同的文件权限。Windows CI 曾复现为 `0o100664`；本机 `rz-formats` 全部 58 项测试及 36 MiB 并行条目往返通过，随后 GitHub CI [40926953354](https://github.com/yuhuotech/rustzip/actions/runs/40926953354) 的 Windows default / all-features 测试通过。
- **图片预览解码边界**：在受支持图像解码前于后台读取文件头，限制 16,777,216 像素，并保留 8 MiB 压缩字节上限；损坏、未知和过大图像继续显示类型卡片。本机完整 GUI 测试、Clippy 通过；GitHub CI [40935152907](https://github.com/yuhuotech/rustzip/actions/runs/40935152907) 七个 job 全部通过。
- **RAR all-features GUI 矩阵**：GUI CI 对 Windows、macOS、Ubuntu/Xvfb 运行 all-features Clippy / 测试；[40939090908](https://github.com/yuhuotech/rustzip/actions/runs/40939090908) 七个 job 全绿，包含 RAR 安全预览和图片尺寸检查。
- **fuzz**：新增 bzip2、RAR fuzz target 和 bzip2 seed。nightly ASan 对 bzip2、CHM、RAR、ZIP recovery 分别运行约 15 秒，约 11,000、21,000、67,000、91,000 次变异，未发现崩溃。短时结果不替代持续 fuzz runner。
- **持续 fuzz runner**：`.github/workflows/fuzz.yml` 设为每日运行五个 nightly ASan target，最多同时跑两个；手动运行时每 target 60 秒，定时运行每 target 180 秒；保留 14 天崩溃 artifact，并按 target 缓存 corpus。`fuzz/seeds/` 保存 ZIP、bzip2、CHM 和 RAR seeds；NSIS seed 由仓库脚本生成。首三轮 run [40907566477](https://github.com/yuhuotech/rustzip/actions/runs/40907566477)、[40908353403](https://github.com/yuhuotech/rustzip/actions/runs/40908353403)、[40909630152](https://github.com/yuhuotech/rustzip/actions/runs/40909630152) 逐步发现并修复 nightly rustc、原生库和 Linux RAR 路径问题。当前代码提交 `171174b` 的手动 run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 五个 target 全部成功并保存 corpus cache：ZIP recovery / CHM / bzip2 / RAR / NSIS 分别运行 315,249 / 60,435 / 8,012 / 187,640 / 3,287,482 次，无 crash artifact。03:23 UTC 的 schedule 未触发；GitHub troubleshooting 指引建议 workflow active 但 schedule 停止时提交 cron 变更重启调度，因此改为每日 05:53 UTC。截至 06:29 仍未见自动 run，需继续观察。

## 本机验证

- `cargo fmt --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；仍有第三方 `block v0.1.6` future-incompatibility 提示。
- `cargo test --workspace --all-features` 通过；2 项需非 root Unix 环境、1 项真实系统钥匙串用例按项目设置忽略。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 通过，并验证基准输出。单次 8 MiB 结果不作通用性能结论。
- ZIP 9.1.3 升级后，`RZ_BENCH_MB=8 cargo bench -p rz-app --bench zip_edit` 的 raw-copy 与 repack 两路均按文件哈希校验通过；单次 8 MiB 结果不作通用性能结论。
- `cargo fuzz run zip_recovery` 在 ZIP 9.1.3 上进行约 91,000 次短时变异，未发现崩溃。
- `cargo deny check --hide-inclusion-graph` 通过 licenses、sources、bans、advisories；仍报告多个传递依赖版本并存。advisory 忽略项都来自上游注明暂无安全升级的依赖。
- 上述依赖审计记录来自本阶段较早的检查。最终改动只将已有传递依赖 `crc32fast` 加为直接依赖；本次环境没有安装 `cargo-deny` 可执行文件，未能重跑依赖审计。该版本的 Cargo.toml 声明 `MIT OR Apache-2.0`，符合项目许可白名单。
- 本机 nightly ASan 下四个 fuzz target 均构建成功；reviewed seed 各运行 5 秒，ZIP recovery / CHM / bzip2 / RAR 分别约 29,605 / 7,995 / 4,462 / 20,552 次变异，未发现崩溃。`actionlint` 检查 `.github/workflows/fuzz.yml` 通过；GitHub run [40912070562](https://github.com/yuhuotech/rustzip/actions/runs/40912070562) 四个 target 均通过并保存 corpus cache。
- GitHub 手动 fuzz run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 对 `171174b` 的五个 target 均成功并保存 corpus cache：ZIP recovery 315,249 次、CHM 60,435 次、bzip2 8,012 次、RAR 187,640 次、NSIS 3,287,482 次；无 crash artifact。新设 05:53 UTC schedule 尚未显示 run。
- `cargo test -p rz-formats --test zip_parallel --all-features` 通过 6 项；36 MiB CJK 名称、重复块伪随机内容分块往返通过本机 `unzip -t`。引擎 40 MiB 文件流式解压和取消清理用例通过；源在第二块后失败时 worker 正常退出。
- `cargo test -p rz-app --lib zip_copy_edit_preserves_archive_and_entry_comments_and_extra_fields` 通过；验证删除和重命名后的归档注释、文件注释及自定义 extra field。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 全矩阵完成并校验输出；ZIP 单条目基准以 BLAKE3 校验 40 / 64 / 128 MiB 样本，每尺寸每设置 3 轮。详细环境和中位数见[基准记录](benchmarks/2026-10-29-zip-large-deflate.md)。
- 2026-10-30 RAR 详情预览改为单条目有界内存读取，继续遵守 `ArchiveReader::open` 64 MiB 兼容上限、`max_total`、声明大小、压缩比和取消检查；通用选择性提取仍使用临时文件以保持大条目流式。新增回归验证输出上限和取消。`cargo fmt --all --check`、全工作区 all-features Clippy、全工作区 all-features 测试通过；三项依赖真实钥匙串或非 root Unix 用户的用例按环境条件忽略。最新三平台 CI 见[第二阶段验收](phase-2-platform-acceptance.md)。

## 后续仍需完成

- ZIP 9.1.3 的 8 MiB 编辑对照基准已运行；仍需多尺寸、多次采样。最新三平台 CI [40926953354](https://github.com/yuhuotech/rustzip/actions/runs/40926953354) 六个 job 全部通过；真实签名 Release 和系统交互验收仍待完成。
- ZIP 大条目分块 Deflate 已接入；本机 40 / 64 / 128 MiB × 3 轮基准通过内容校验，仍需 Windows / macOS 外部工具矩阵和其他文件类型性能样本。合格 ZIP 编辑快路径的注释和扩展字段保真见 D42；真正原地增量编辑仍需先处理 D11 的崩溃保全边界。
- 持续 fuzz 已在 GitHub Actions 手动运行五个 target 并保存五份 corpus cache；每日自动 schedule 在 03:23 UTC 未触发，cron 已重注册为 05:53 UTC 但截至 06:29 仍无 schedule 事件，需继续观察；未来 crash artifact 流程尚未由真实 crash 验收。
- WIM / DMG 仍只有签名识别；NSIS 仅有受限 FirstHeader 识别。三种格式的浏览 / 测试 / 提取 reader 仍需通过 D37。
- PDF / 音频 / 视频应用内预览，关机等高影响完成后动作。
- 压缩包对比、虚拟挂载、云端按需解压、WASM 插件和 SFX 仍为远期需求。
- macOS Finder Sync 右键菜单已在本机 ad-hoc 包实测通过；当前 HEAD 的四平台常规 / RAR 测试包、Windows RAR 安装冒烟及 manifest 验签在 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 均已通过。正式签名 Release 和真实用户交互仍待完成，详见[发布验收](release.md)。
