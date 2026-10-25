# RustZIP S0 基线、S1 首批与 S2 定向执行报告

日期：2026-10-25。
环境：macOS 27.0 arm64（Apple Silicon、APFS 默认大小写不敏感卷），`rustc 1.96.0`、`cargo-llvm-cov 0.9.1`。
范围：[方案](testing-stability-plan.md) S0（default/all-features 覆盖基线与测试核对）+ S1 首批 P0（FS-01/03/04/05/06/07/10、TASK-03/04/06、LOC-07、SVC-04、SVC-09、TRANS-02/06、PERF-01 的基准部分）+ S2 GUI-02/LOC-06 异步竞态首批。
关联决策：[D11](decisions.md)；S2 异步交互合同见 D14。

## 结论

- **全 workspace 回归**：default 为 320 通过 / 3 忽略 / 0 失败；`--all-features` 为 326 通过 / 3 忽略 / 0 失败。覆盖运行包含 GUI 与 CLI，排除仅作探针的 `gpui-probe`。被忽略的两项权限测试随后在当前非 root 账号下单独运行并通过；系统钥匙串测试仍忽略。
- **覆盖率基线**：`cargo llvm-cov` 分别采集 default 与 `--all-features`。全 workspace 行覆盖率分别为 **85.41%**（11,945/13,985）与 **85.50%**（12,060/14,106）；区域覆盖率 84.19% / 84.23%，函数覆盖率 76.44% / 76.56%。按 crate 的行覆盖率：`rz-app` 95.5%、`rz-core` 93.8%、`rz-engine` 92.3%、`rz-formats` 91.5–91.9%、`rz-platform` 90.1%、`rz-services` 55.3%、GUI 79.6–79.7%。当前采集不含分支计数，不能把行覆盖率解释为分支覆盖率。
- **规范检查**：`cargo fmt --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- **性能**：改动前后（HEAD `fa98dc4` worktree 对比）`RZ_BENCH_MB=32` 基准全部格式吞吐差在 ±11% 运行噪声内，无系统性回归。
- **发现并修复的实现缺陷**（均为测试先行，修复后通过；详见 [D11](decisions.md)）：
  1. compress 会用固定名 `.rz-part` 并在结束时无条件 rename 覆盖已有目标（FS-03/06）。
  2. compress 发布失败会遗留临时文件（FS-01）。
  3. 解压会跟随目标目录里预置的符号链接（父目录、目标文件、目录条目、以及"刚创建的目录被换成链接"的 TOCTOU——FS-04/05）。首版修复曾引入"已验证目录"缓存，被 FS-05 用例当场击穿后移除。
  4. Replace 模式解压在读取失败/取消时先截断再删除旧文件（FS-07）。
  5. 同进程同包并发编辑共享同一暂存目录名并互相删除、后发布者静默覆盖前者的修改（TASK-04 丢更新）。
  6. 设置保存用固定 `.tmp` 名，并发保存交错可产生损坏文件（LOC-07）。
  7. `CreateOptions`/`ExtractOptions` 的派生 Debug 会打印明文密码（SVC-09）。
  8. 基准错误只打印不退出、吞吐不做内容校验（PERF-01）。
  9. 并行 ZIP writer 在排队中途收到错误时，会在线程作用域 join 之后才关闭任务通道，空闲 worker 因此永远等不到退出；取消/暂停风暴可让 `TaskManager::drop` 卡住（TASK-03）。修复为错误传播前关闭发送端，并新增有超时的子进程回归用例。

## 逐用例记录

| 用例 | 状态 | 变体与证据 |
|---|---|---|
| FS-01 | 通过（注入范围） | 压缩覆盖中途源读取失败、归档 writer 中途失败和 finish 失败，均不发布且清临时；设置保存另有短写、sync、publish 故障注入并保留最后有效文件。归档写入尚无真实 ENOSPC/底层文件 sync 故障注入。原权限测试在本机非 root 环境单独运行通过。 |
| FS-03 | 通过（本机变体） | 压缩拒绝已有目标、发布前目标被创建；新增压缩目标为源文件符号/硬链接别名的用例。转换同样覆盖符号/硬链接别名及发布前目标抢占，源和对方创建的目标均保持原样。 |
| FS-04 | 通过（unix） | `security.rs::link_defense`：父目录链接、目标文件链接、目录条目落链接，哨兵字节不变。Windows junction：思路一致（junction 在 Rust 中表现为 symlink），**环境未覆盖**（无 Windows CI 运行）。 |
| FS-05 | 通过（unix） | `a_folder_swapped_for_a_link_after_the_preflight_is_refused`：mock reader 在目录条目与文件条目之间换链，写入拒绝，外部目录仅剩原文件。 |
| FS-06 | 通过 | 预置外部 `.rz-*.part` 不被读取/截断；双线程同目标并发 5 轮：恰好一个成功、输出有效、无残留。 |
| FS-07 | 通过 | Replace + 损坏条目 / 取消保全旧文件；注入 mtime 失败时旧文件仍在提交点前保全；只读 mode 与时间戳组合在写入顺序正确时可恢复。Skip 竞态按跳过处理。 |
| FS-10 | 通过 | `edit_failures.rs` 子进程 SIGKILL：解包/打包中击杀 → 原包字节不变；发布后击杀 → 新版本完整可读、可解压；转换中击杀 → 无输出、源不变。残留仅可识别的 `.rz-` 前缀，且不阻塞重试。 |
| TASK-02 | 通过（部分） | `tasks.rs::a_cancel_and_pause_storm...`：10 任务 × 双线程风暴，恰一终态、事件序合法、终态后操作拒绝。参数化"各阶段暂停/恢复"矩阵未展开（P1 余项）。 |
| TASK-03 | 通过 | `dropping_the_manager_mid_cancel_storm_never_hangs` 与 `a_cancel_and_pause_storm_ends_with_one_terminal_state_per_task` 通过；取消风暴曾复现 ZIP worker join 卡死。新增 `zip_parallel::an_early_parallel_error_closes_idle_workers_before_join`，子进程 10s 监督验证早期错误会关闭任务通道、worker 全部退出。 |
| TASK-04 | 通过（本机进程模型） | 同进程并发编辑与编辑 × 转换；新增两个真实子进程同时编辑同包，两次删除均保留且包可读。按归档路径使用 OS advisory lock 串行化，等待锁期间取消可退出。网络文件系统及不同 OS 锁语义未覆盖。 |
| TASK-06 | 通过（部分） | `transform_failures.rs::a_cancellation_at_any_phase_never_publishes`（10/50/90% 进度取消 → 无发布、无临时）+ `io_failures.rs` 压缩取消清理 + `roundtrip.rs` 既有取消用例。恰在 finish/verify/rename 指令级边界的取消需要引擎检查点接口，未实施。 |
| LOC-07 | 通过（应用层） | 目标被目录占用、并发与 50 次连发保存，以及短写、sync、publish 故障注入：最后有效设置可读、临时文件清理。GUI 快速切换后立即退出的调度变体仍属 S2。 |
| SVC-04 | 通过 | `password_failures.rs`：set 成功/索引失败、delete 成功/索引失败 → 错误可见、状态真实可恢复；8×25 并发无丢失。真实钥匙串半成功事务（GUI keychain.rs）未覆盖。 |
| SVC-09 | 通过 | Debug 输出 `<redacted>`（经 JobSpec 组合验证）；错误串与 TaskError 不含密码；find_password 行为。完整用户路径扫描（日志面）未系统化。 |
| TRANS-02 | 通过 | 同路径/已存在/符号链接别名/硬链接别名、只读目录、发布前输出被其他线程抢先创建；源字节不变，竞争方输出未被覆盖且临时文件清理。 |
| TRANS-06 | 通过 | TMPDIR 指向只读目录（子进程）→ 修复整体失败、无输出；对照：坏 CRC 条目 → 跳过并报告。 |
| PERF-01 | 通过 | 基准改为错误非零退出 + 解压内容校验（文件数、大小、样本字节）后才记录吞吐；两份运行（HEAD 与当前）对比无回归。 |

## 未复现的单次失败

第一次 all-features 覆盖运行中，`roundtrip::overwrite_policies` 在一次 default-Rename 解压时返回 `AlreadyExists`。没有改动解压策略；随后该用例单独重复 20 次、roundtrip 全组执行一次、default 覆盖运行和 all-features 覆盖重跑均通过。记录为**未复现的瞬时失败**；若再次出现需保留新日志并继续定位，不能把它当成已修复缺陷。

## S2 定向异步竞态

| 用例 | 状态 | 变体与证据 |
|---|---|---|
| GUI-02 | 通过（可控完成顺序） | 新增 `out_of_order_archive_open_completions_keep_the_latest_request_active`：按 B 完成、A 完成的顺序交付真实解包树，两个标签都保留且 B 仍是当前标签；同一路径的重复未完成请求合并。显式切换标签、页面或文件夹会使旧打开请求失去焦点资格。窗口销毁期间真实后台回调仍需原生生命周期验收。 |
| LOC-06 | 通过（代次控制） | 新增 `location_state::tests::an_old_folder_scan_cannot_restore_stale_items_or_selection`：旧扫描在新扫描之后返回时不回写列表、不清除新扫描 loading；最新结果应用后移除过期选择。网盘断开与真实权限拒绝区分仍未覆盖。 |

GUI 无头整组共 52 项通过，GUI 模型单测 18 项通过；这批只验证回调顺序与代次，不代表 GUI-01/03/05–14、原生窗口和系统文件选择器已验收。

## 明确未覆盖 / 待后续阶段

- Windows（junction、保留名、文件占用）、Linux（XDG、只读挂载）真机矩阵：环境未覆盖。
- `TASK-06` 在 finish、输出验证、最终 rename/publish 指令边界的取消检查点仍未单独注入；当前取消用例覆盖 10/50/90% 写入进度与中途失败。
- GUI 异步回调/快速保存退出矩阵（S2）、格式外部 oracle 与更完整 RAR 样本矩阵（S3）、Linux/Windows 原生环境仍待执行。
- 真实系统钥匙串往返及半成功操作未覆盖；因既有真实钥匙串用例会卡住，本轮继续保持 ignored。SVC-09 的完整路径脱敏扫描也未系统化。
- 分支覆盖仍未知；当前 stable 工具链报告只提供行、区域和函数数据。

## 覆盖率复现

```sh
cargo llvm-cov --workspace --exclude gpui-probe --quiet --json --output-path /tmp/rustzip-coverage-default-final.json
cargo llvm-cov --workspace --all-features --exclude gpui-probe --quiet --json --output-path /tmp/rustzip-coverage-all-features-final.json
```

以上报告覆盖当前工作树，不只是 `HEAD fa98dc4`；JSON 留在本机 `/tmp`，未上传外部服务。行覆盖率包含应用与 GUI 源码，测试夹具和被排除的 `gpui-probe` 不计入分母。
