# 第四阶段：解压安全、并行编解码与依赖修复

开始日期：2026-10-29。基线提交：`8718b05`。本阶段从路线图和平台验收记录继续，保留尚未满足安全条件的格式为“不支持”。

## 已完成

- **RAR 输出限制**：UnRAR 的逐块数据回调封装在 `rz-ffi`。通用选择性提取的选中条目按声明大小、压缩比和剩余 `max_total` 限量写入单条目临时文件；未选条目测试解码但不落临时盘；解码中可取消。整条目 `ArchiveReader::open` 读取限制为 64 MiB，单条目通过有界内存缓冲读取，不写临时盘；GUI 预览另有 8 MiB 图片 / 64 KiB 文本限制。默认包仍关闭 RAR。本轮新增双条目 fixture，验证累计预算耗尽后不会解码第二个选中条目；CI [41097739228](https://github.com/yuhuotech/rustzip/actions/runs/41097739228) 的三平台 all-features core 测试通过。见决策 D38。
- **bzip2 并行解压**：通过 MIT `pbz2` 0.2.0 解码 bzip2 单流和串接 member，按配置线程数和内存预算估算 worker，预算不足时回退 `MultiBzDecoder`。本地 vendor 补丁让 worker / scanner 启动故障返回错误；随包携带 MIT license 与 NOTICE。
- **gzip 分块 Deflate**：`.gz` / `.tar.gz` 多线程压缩按最多 900,000 字节分块，生成按顺序连接的标准 gzip members；低预算时回退 flate2。
- **ZIP 大条目分块 Deflate**：未加密 Deflate 条目按 900,000 字节分块并行压缩，按输入顺序写入有界匿名临时 ZIP，再由 `zip` 9.1.3 合并压缩数据。worker 数与每 worker 两个待处理块按 16 MiB / worker 估算预算；Store、加密及单 worker 情况沿用顺序 writer。见决策 D41。
- **ZIP 依赖升级**：工作区统一升级到 `zip` 9.1.3，替换受安全公告影响的 2.6.1 与旧 zstd 依赖。应用解压继续走 RustZIP 引擎安全路径。cargo-deny 配置见仓库根目录 `deny.toml`；无安全升级的上游维护状态公告保留有原因的忽略项。
- **ZIP 编辑元数据保真**：快路径放开归档 / 条目注释输入，复制时保留归档注释、ZIP64 扩展数据及未修改条目的文件注释 / 扩展字段；拒绝带自解压前缀的快路径输入。通过删除、重命名后逐字节检查注释和自定义 extra field。见 D42。
- **ZIP 跨平台权限**：便携模式明确写入 Unix creator 标记和目录 `0755` / 文件 `0644`，不再依赖运行平台的 ZIP 默认值；并行大条目 Deflate 使用相同的文件权限。Windows CI 曾复现为 `0o100664`；本机 `rz-formats` 全部 58 项测试及 36 MiB 并行条目往返通过，随后 GitHub CI [40926953354](https://github.com/yuhuotech/rustzip/actions/runs/40926953354) 的 Windows default / all-features 测试通过。
- **图片预览解码边界**：在受支持图像解码前于后台读取文件头，限制 16,777,216 像素，并保留 8 MiB 压缩字节上限；损坏、未知和过大图像继续显示类型卡片。本机完整 GUI 测试、Clippy 通过；GitHub CI [40935152907](https://github.com/yuhuotech/rustzip/actions/runs/40935152907) 七个 job 全部通过。
- **RAR all-features GUI 矩阵**：GUI CI 对 Windows、macOS、Ubuntu/Xvfb 运行 all-features Clippy / 测试；[40939090908](https://github.com/yuhuotech/rustzip/actions/runs/40939090908) 七个 job 全绿，包含 RAR 安全预览和图片尺寸检查。
- **fuzz**：新增 bzip2、RAR fuzz target 和 bzip2 seed。nightly ASan 对 bzip2、CHM、RAR、ZIP recovery 分别运行约 15 秒，约 11,000、21,000、67,000、91,000 次变异，未发现崩溃。短时结果不替代持续 fuzz runner。
- **crash artifact smoke**：手动 Fuzz workflow 新增默认关闭的 `verify_crash_artifact` 选项和独立 sentinel target。显式开启时，它触发一次受控 libFuzzer crash，要求生成非空 `crash-*` 并通过 14 天 artifact upload；正常定时任务不运行此 target。本机 nightly ASan 与 GitHub run [41046064115](https://github.com/yuhuotech/rustzip/actions/runs/41046064115) 均验证了该路径；run 的六个 job 全部成功，下载得到 492-byte `fuzz-crash-artifact-smoke-41046064115`，其中 crash 文件内容与 31-byte sentinel 相同。
- **构建期统计地址验证**：Release 每个平台先构建 `rz-services` 测试子进程，并移除运行时 `RZ_STATS_URL`，确认 `stats_url()` 实际返回构建期地址。Release dry-run [41064921267](https://github.com/yuhuotech/rustzip/actions/runs/41064921267)、[41079816983](https://github.com/yuhuotech/rustzip/actions/runs/41079816983) 与最新 main 的 [41090713832](https://github.com/yuhuotech/rustzip/actions/runs/41090713832) 均通过四平台包 / RAR 验证及汇总验签；最新 run 也通过 Windows RAR 安装冒烟，公开发布按 dry-run 跳过。
- **持续 fuzz runner**：`.github/workflows/fuzz.yml` 设为每日运行五个 nightly ASan target，最多同时跑两个；手动运行时每 target 60 秒，定时运行每 target 180 秒；保留 14 天崩溃 artifact，并按 target 缓存 corpus。`fuzz/seeds/` 保存 ZIP、bzip2、CHM 和 RAR seeds；NSIS seed 由仓库脚本生成。首三轮 run [40907566477](https://github.com/yuhuotech/rustzip/actions/runs/40907566477)、[40908353403](https://github.com/yuhuotech/rustzip/actions/runs/40908353403)、[40909630152](https://github.com/yuhuotech/rustzip/actions/runs/40909630152) 逐步发现并修复 nightly rustc、原生库和 Linux RAR 路径问题。提交 `171174b` 的 run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 五目标均通过；`8acf65e` 的 run [41023957604](https://github.com/yuhuotech/rustzip/actions/runs/41023957604) 五目标也通过：ZIP recovery / CHM / NSIS / RAR / bzip2 分别 287,292 / 66,874 / 1,674,414 / 118,009 / 5,041 次；提交 `fabff8b` 的 run [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 五目标再通过，分别运行 265,079 / 54,151 / 2,639,918 / 137,390 / 5,460 次。提交 `6fd8e65` 的 [Fuzz run 41079856580](https://github.com/yuhuotech/rustzip/actions/runs/41079856580) 五个 target 与受控 crash artifact smoke 全部成功。定时触发尝试在 2026-10-30 13:37、16:53 和 17:35 UTC 均未生成 `schedule` run；workflow 在 17:11 UTC 重新注册为 active，cron 恢复为 13:37 UTC，手动 dispatch 不作为定时验收。

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
- GitHub 手动 fuzz run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 对 `171174b` 的五个 target 均成功并保存 corpus cache：ZIP recovery 315,249 次、CHM 60,435 次、bzip2 8,012 次、RAR 187,640 次、NSIS 3,287,482 次；无 crash artifact。其后 05:53、07:37、09:37 与 11:37 UTC schedule 均无 run；当前定时为 13:37 UTC，等待本次 actor 重注册后的实际事件。
- GitHub 手动 fuzz run [41023957604](https://github.com/yuhuotech/rustzip/actions/runs/41023957604) 在 `8acf65e` 上五个 target 全部成功并保存 corpus cache：ZIP recovery 287,292 次、CHM 66,874 次、NSIS 1,674,414 次、RAR 118,009 次、bzip2 5,041 次；各 job 成功且没有 crash artifact。
- GitHub 手动 fuzz run [41026029882](https://github.com/yuhuotech/rustzip/actions/runs/41026029882) 在 `fabff8b` 上五个 target 全部成功并保存 corpus cache：ZIP recovery 265,079 次、CHM 54,151 次、NSIS 2,639,918 次、RAR 137,390 次、bzip2 5,460 次；各 job 成功且没有 crash artifact。
- GitHub 手动 Fuzz run [41046064115](https://github.com/yuhuotech/rustzip/actions/runs/41046064115) 对提交 `574a2f6` 的五个常规 target 和 crash artifact smoke 六个 job 全部通过；artifact `fuzz-crash-artifact-smoke-41046064115` 已下载并检查，包含预期 sentinel crash 文件。该验证证明 artifact 上传/保留路径，不计为格式崩溃。
- `09:37` 与 `11:37 UTC` 定时均未触发。schedule 已移到每日 `13:37 UTC`，并在默认分支更新后禁用再启用 workflow，以当前写入者 actor 重新注册。
- `cargo test -p rz-formats --test zip_parallel --all-features` 通过 6 项；36 MiB CJK 名称、重复块伪随机内容分块往返通过本机 `unzip -t`。引擎 40 MiB 文件流式解压和取消清理用例通过；源在第二块后失败时 worker 正常退出。
- `cargo test -p rz-app --lib zip_copy_edit_preserves_archive_and_entry_comments_and_extra_fields` 通过；验证删除和重命名后的归档注释、文件注释及自定义 extra field。
- `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 全矩阵完成并校验输出；ZIP 单条目基准以 BLAKE3 校验 40 / 64 / 128 MiB 样本，每尺寸每设置 3 轮。详细环境和中位数见[基准记录](benchmarks/2026-10-29-zip-large-deflate.md)。
- 2026-10-30 本轮在锁定 Rust 1.96.0 下完成验证：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features` 均通过。GUI 编码设置持久化回归、RAR FFI 回调 4 项、RAR 引擎 8 项及全工作区测试通过；4 项真实钥匙串或非 root Unix 用例按环境条件忽略。Clippy 仍提示第三方 `block v0.1.6` future-incompatibility。
- 2026-10-30 密码本自动尝试接入任务密码失败路径后，再次使用锁定 Rust 1.96.0 执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 和 `cargo test --workspace --all-features`，均通过。GUI 全部 65 项 UI 集成用例通过，覆盖加密 ZIP 候选重试和候选不匹配后的手动重试；RAR FFI 4 项及引擎 8 项通过。两项非 root Unix 用例和真实钥匙串用例按环境条件忽略。Clippy 仍提示第三方 `block v0.1.6` future-incompatibility。
- 2026-10-31 本机切换到明确的 Rust 1.96.0 编译器和隔离 target 后，`cargo fmt --all -- --check`、全工作区 all-features Clippy 和 `cargo test --workspace --all-features` 均通过。视频 renderer 7 项、helper 资源限制 3 项、RAR FFI 4 项和 RAR 引擎 9 项全部通过；按环境条件忽略项由测试输出注明。此前共享 target 混有 Rust 1.99.1 元数据，未清理原缓存，改用独立目录完成验证。Clippy 提示第三方 `block v0.1.6` future-incompatibility。

## 后续仍需完成

- ZIP 9.1.3 的 8 MiB 编辑对照基准已运行；仍需多尺寸、多次采样。最新三平台 CI [41086116233](https://github.com/yuhuotech/rustzip/actions/runs/41086116233) 七个 job 全部通过，含三平台 core / GUI、Windows 安装器 smoke 和 benchmark；真实签名 Release 和系统交互验收仍待完成。
- ZIP 大条目分块 Deflate 已接入；本机 40 / 64 / 128 MiB × 3 轮基准通过内容校验，仍需 Windows / macOS 外部工具矩阵和其他文件类型性能样本。D53 的 ZIP staged append / reflink 路径保留 D11 的暂存、校验和原子替换边界；macOS reflink 卷上的 8 / 40 / 128 MiB × 3 轮编辑基准已通过内容哈希核对，结果见[ZIP staged incremental 基准](benchmarks/2026-10-31-zip-staged-incremental.md)。Windows / Linux 外部工具和性能验收仍待完成，原文件原地改写仍未实现。
- Fuzz run [41134178445](https://github.com/yuhuotech/rustzip/actions/runs/41134178445) 的七个普通 target 通过，`video_decoder` 发现 MP4 sample-table panic；本地加入 box-tree、extended-size、`stsc` / `stts` 覆盖校验和三个回归 seed 后，nightly ASan 30 秒完成 61,003 次变异、峰值 RSS 441 MiB，无崩溃。修复后的远端结果待验收。workflow 已配置每日 23:30 UTC，schedule 触发结果待观察。
- WIM / DMG 仍只有签名识别；NSIS 仅有受限 FirstHeader 识别。三种格式的浏览 / 测试 / 提取 reader 仍需通过 D37。
- PDF、音频和视频首屏 / 播放能力已实现；剩余 PDF / 音视频 helper 的三平台安装与运行、Finder 空格键可见页面及 Windows 原生交互验收。关机等高影响完成后动作仍不纳入当前版本。
- 压缩包对比、虚拟挂载、云端按需解压、WASM 插件和 SFX 仍为远期需求。
- macOS Finder Sync 右键菜单已在本机 ad-hoc 包实测通过；提交 `fabff8b` 的 [CI 41026008129](https://github.com/yuhuotech/rustzip/actions/runs/41026008129) 七个 job 全部通过，[Release dry-run 41026007832](https://github.com/yuhuotech/rustzip/actions/runs/41026007832) 的四平台常规 / RAR 测试包、Windows RAR 安装冒烟及 manifest 验签也全部通过。正式签名 Release 和真实用户交互仍待完成，详见[发布验收](release.md)。
