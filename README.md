# RustZIP

用 Rust 开发的跨平台压缩管理软件(Windows / macOS / Linux),追求极致性能和简洁美观的体验。

项目文档见 [`docs/`](docs/):产品需求、技术架构、技术规范、UI 规范、决策记录。AI 协作规则见 [`AGENTS.md`](AGENTS.md)。

## 当前进度(M0)

| 能力 | 状态 |
|---|---|
| zip / tar / tar.gz 读写 | 已完成 |
| zip AES-256 加密与解密 | 已完成 |
| 安全解压(路径穿越、压缩炸弹、符号链接、setuid) | 已完成 |
| 取消、进度回调、原子写入(失败不留半成品) | 已完成 |
| 权限与修改时间保留 | 已完成 |
| 命令行 `rz a / l / x / t` | 已完成 |
| 7z、xz、zstd、bzip2、rar(读) | 未开始 |
| 并行流水线 | 未开始 |
| GUI(待 UI 框架验证) | 未开始 |

## 命令行

```
rz a <archive> <path>... [-l 0-9] [-p password]   创建(.zip .tar .tar.gz .tgz)
rz l <archive>                                    列出内容
rz x <archive> [-o dir] [-p password] [-y|-n]     解压(-y 覆盖,-n 跳过已有)
rz t <archive> [-p password]                      校验完整性
```

## 开发

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p rz-engine
```

许可证:`MIT OR Apache-2.0`。
