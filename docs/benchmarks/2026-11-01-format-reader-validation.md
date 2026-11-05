# 格式 reader 本机基准与对照

- 环境：macOS arm64、15 个逻辑 CPU、Rust 1.96；当前工作区与 `f17e1bd` 的独立源码快照使用同一套基准和同一台机器。
- 命令：`RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput`，分别在两个源码版本上运行；另在当前代码运行 `RZ_BENCH_MB=8 cargo bench -p rz-app --bench zip_edit`。基准在报告吞吐之前检查文件内容或散列。
- 8 MiB 样本每个版本各运行一轮；ZIP 大文件 40 / 64 / 128 MiB 每档各三次。当前基准运行时存在其他构建及 Fuzz，不能视为严格隔离的性能回归检验。

| 指标 | `f17e1bd` | 当前工作区 |
|---|---:|---:|
| ZIP 8 MiB 单线程压缩 / 解压 | 73 / 1428 MiB/s | 70 / 1388 MiB/s |
| ZIP 8 MiB 15 线程压缩 / 解压 | 338 / 3405 MiB/s | 394 / 3623 MiB/s |
| tar.gz 8 MiB 单线程压缩 / 解压 | 80 / 926 MiB/s | 80 / 916 MiB/s |
| 7z 8 MiB 单线程压缩 / 解压 | 40 / 235 MiB/s | 38 / 234 MiB/s |
| ZIP 64 MiB 单线程压缩中位数 | 84 MiB/s | 84 MiB/s |
| ZIP 64 MiB 15 线程压缩中位数 | 835 MiB/s | 838 MiB/s |
| ZIP 128 MiB 15 线程压缩中位数 | 892 MiB/s | 907 MiB/s |

ZIP 暂存增量编辑在当前 APFS reflink 卷、保留 8 MiB 文件、三轮中位数为 0.012 秒；强制完整重打包为 0.115 秒。输入是重复伪随机块，仅反映本机该场景。

DMG CRC 校验接入后再次运行完整基准并执行单次源码对照：8 MiB ZIP 单线程压缩 / 解压分别为旧版 63 / 1328、当前 78 / 1355 MiB/s；64 MiB、15 线程 ZIP 压缩三次中位数为旧版 870、当前 841 MiB/s；128 MiB 对应旧版 916、当前 887 MiB/s。此前一次与交叉编译并发的 8 MiB ZIP 解压下降明显，不能与单独采样直接比较。最终编辑基准的三次中位数为增量 0.013 秒、完整重打包 0.117 秒。

这批结果用于确认现有基准和输出校验可运行，样本不足、并发干扰以及单机条件均不足以证明无性能回退。新 WIM / DMG / NSIS 的吞吐和真实大归档内存使用尚未建立独立跨平台基线。
# 2026-11-03 补充：APFS 透明压缩

本机运行 `RZ_BENCH_MB=8 cargo bench -p rz-engine --bench throughput` 成功。
该基准主要覆盖 ZIP、7z 和 TAR 等已有路径，并非 APFS 专项；运行时有其他编译
任务并行，因此不能据此判定性能无回归。APFS 新路径的独立吞吐基线留待集成验收。

随后新增 `cargo bench -p rz-formats --bench apfs_compressed`，本机串行运行
1000 次真实 macOS 生成的 type 8 LZVN 资源分叉读取，每次验证完整输出：
171.7 MiB / 0.054 秒，约 3166 MiB/s。样本仅 180000 字节且缓存热，此数值是
可重复的专项基线，不代表大镜像、冷盘或跨平台性能。
