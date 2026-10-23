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
| 命令行 `rz a / l / x / t` | 已完成 |
| 7z 分块并行解压、bzip2 多线程、大文件分块 deflate | 未开始 |
| 分卷、修复、格式转换、压缩包内编辑 | 未开始 |
| GUI(待 UI 框架验证) | 未开始 |

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
rz x <archive> [-o dir] [-p password] [-t threads] [-y|-n]     解压(-y 覆盖,-n 跳过已有)
rz t <archive> [-p password]                                   校验完整性
```

创建格式由扩展名决定:`.zip .7z .tar .tar.gz .tar.xz .tar.zst .tar.bz2`(及 `.tgz .txz .tzst .tbz2`),
单文件 `.gz .xz .zst .bz2`。读取时按文件头识别,不看扩展名。

## 开发

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo bench -p rz-engine
```

许可证:`MIT OR Apache-2.0`(启用 `rar` feature 的构建还需遵守 unRAR 的许可条款)。
