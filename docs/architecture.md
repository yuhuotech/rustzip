# 技术架构

## 1. 分层

```
┌────────────────────────────────────────────────────┐
│ 表现层  GUI │ CLI │ Shell 扩展(各平台原生)              │
├────────────────────────────────────────────────────┤
│ 应用服务层  rz-app:任务队列、事件总线、哈希、设置、密码库、更新、统计 │
├────────────────────────────────────────────────────┤
│ 引擎层  rz-engine:流水线、调度、内存预算、安全策略           │
├────────────────────────────────────────────────────┤
│ 格式层  rz-formats:统一 Archive trait,各格式后端          │
├────────────────────────────────────────────────────┤
│ 编解码层  rz-codecs / rz-crypto:压缩算法、加密、校验        │
├────────────────────────────────────────────────────┤
│ 平台层  rz-platform:文件系统、权限、符号链接、ADS、xattr    │
└────────────────────────────────────────────────────┘
```

依赖方向只能自上而下。下层不得依赖上层。GUI 和 CLI 只通过 `rz-app` 使用核心能力,不直接调用格式层。CLI 的同步命令门面也由 `rz-app::cli` 提供，内部统一委托引擎和格式层。

## 2. Workspace 结构

```
rustzip/
├─ crates/
│  ├─ rz-core        # 错误类型、Archive/Entry trait、进度与取消 token
│  ├─ rz-codecs      # 压缩算法封装(统一 Encoder/Decoder 接口)
│  ├─ rz-formats     # zip / 7z / tar / rar(读) / libarchive 兜底
│  ├─ rz-engine      # 并行流水线、解压安全检查、增量更新
│  ├─ rz-crypto      # AES、KDF、校验和
│  ├─ rz-vfs         # 把压缩包抽象为虚拟文件系统(浏览、预览、挂载)
│  ├─ rz-platform    # 平台差异(Win/mac/Linux)
│  ├─ rz-app         # 任务队列、设置、密码库、更新、统计,GUI 与 CLI 共用
│  └─ rz-ffi         # libarchive 等 C 库绑定
├─ apps/
│  ├─ cli            # rz 命令
│  ├─ gui            # 桌面应用
│  └─ shell-ext/     # windows(COM) / macos(Finder Sync + Quick Look) / linux(Nautilus、Dolphin)
├─ docs/             # 产品、架构、技术规范、UI 规范、决策记录
├─ benches/          # 性能基准
└─ fuzz/             # 模糊测试
```

## 3. 格式后端:三层兜底

1. **纯 Rust 原生,读写**:zip、tar 系、gz、xz、zst、bz2、lz4、7z。基于 `zip`、`tar`、`flate2`(zlib-rs 后端)、`zstd`、`liblzma`、`lz4_flex`、`sevenz-rust2` 等 crate；LZ4 单文件流和 tar.lz4 容器使用标准 LZ4 Frame 格式。性能关键路径(7z 固实块、多线程 LZMA2)自己做调度,不直接套用单线程接口。
2. **绑定成熟 C 库,只读**:RAR 用 `unrar`(做成 Cargo feature);ISO 9660、CAB、CPIO、AR、LHA/LZH、XAR 和 RPM 用 libarchive。安全封装放在 `rz-ffi`,只注册实际验收的格式及必要的内置滤镜，不启用 libarchive 的 RAR、ZIP 或 7z 解码器，也不调用外部解压命令。
3. **纯 Rust 专用只读后端**:CHM 通过 MIT `libchm` 的本地补丁版读取；`rz-formats::chm_fmt` 用 64 KiB caller buffer 流式读取，并在 LZX block 边界检查取消。目录元数据、reset table 和压缩 block 都有硬上限，详情见[第三阶段评估](phase-3-format-assessment.md)。
4. **格式嗅探**:按魔数识别,不按扩展名;支持嵌套识别。WIM、DMG 目前仅识别签名并返回“不支持”；NSIS 不由通用 PE / `.exe` 特征推断。新增 reader 必须先通过许可证、流式读取、取消、资源上限与独立样本验收。

统一接口(草案):

```rust
trait ArchiveReader {
    fn entries(&self) -> Box<dyn Iterator<Item = Result<Entry>> + '_>;
    fn open(&self, entry: &Entry) -> Result<Box<dyn Read + Send>>;
    fn capabilities(&self) -> Caps; // random_access / solid / encrypted / writable
}
trait ArchiveWriter { /* add / finish / 增量更新 */ }
```

`Caps` 让引擎知道某种格式能否随机访问,据此选择并行策略。

## 4. 性能设计

- **并行模型**:Rayon 加有界 channel 组成流水线:`读盘 → 分块 → 压缩/解压 → 校验 → 写盘`。
  - zip 有中央目录,按文件并行。
  - 7z 固实块之间并行,块内用 LZMA2 多块切分并行。
  - tar 系用分块并行(如 `zstd` 多线程)。
- **I/O**:大文件用 mmap 或带预读的缓冲;小文件批量合并以减少系统调用;Linux 可选 io_uring,Windows 可选 overlapped I/O。
- **内存预算**:全局内存上限,字典大小和线程数联动,避免 OOM。
- **算法与硬件加速**:CRC 用 SIMD(`crc32fast`);zip 压缩优先 zlib-rs 或 libdeflate;校验可选 BLAKE3。
- 文件 SHA-256 / BLAKE3 由 `rz-app::hash` 按 256 KiB 块流式计算；一次读取可更新两种算法，并共享任务取消和进度事件。
- 7z LZMA2 支持显式字典和最大固实块大小；格式层根据字典、输入块缓存和线程数估算工作集，先降低并行度并约束分块。自动字典在预算内下调，显式字典超预算则拒绝创建。
- 7z 解压在多个独立固实块上并行调度，每块保持顺序解码；目前只对 COPY、LZMA2 和 AES 编码组合估算每个工作线程的字典和开销，默认使用 256 MiB 工作预算。字典超预算或遇到未估算编码时回退顺序解码；预算是并行调度估算，不是对整个进程的硬内存限制。
- bzip2 压缩将输入分为最多 900,000 字节的块，独立压缩成标准 bzip2 流后按输入顺序拼接；工作线程和在途块数量受 `CreateOptions::memory_budget` 估算约束，预算不足时使用单线程编码。读取继续使用支持连续流的 `MultiBzDecoder`。
- ZIP 编辑对符合条件的删除、重命名和文件写回复用未更改文件的压缩数据，并保留路径安全、原包与新包完整性校验和同目录原子替换；其他编辑和不支持的输入继续采用完整解压重打包。该优化重写整个容器但不重新压缩未改动的文件，并非原地增量写入。
- RAR 后端在返回条目流前会完整写出临时文件；应用层拒绝 RAR 的预览读取，避免预览绕过磁盘输出上限。正常解压不受此预览限制影响，见 D35。
- **零拷贝与流式**:内部不整体缓冲文件;格式转换和云读取全程流式。
- **大目录**:压缩包浏览惰性加载加虚拟列表,百万级条目不卡。
- **基准**:`benches/` 对标 7-Zip、`tar | zstd`,CI 里做性能回归检测。

## 5. 安全设计

- **防 Zip Slip**:禁止 `../`、绝对路径、盘符、符号链接逃逸;规范化 Windows 保留名。
- **防压缩炸弹**:限制压缩比、总解压大小、嵌套深度,超限时提示用户。
- **模糊测试**:所有解析器对不可信输入做 `cargo-fuzz`。
- **平台属性**:权限位、symlink、xattr、NTFS ADS、macOS quarantine 标记按策略保留(解压出的可执行文件应继承 quarantine)。
- **密码**:内存中短暂保留并用 `zeroize` 清除;持久化走系统钥匙串。

## 6. GUI 技术

- **首选:GPUI Kit**(`longbridge/gpui-kit`,基于 Zed 的 GPUI,Apache-2.0)。纯 Rust、GPU 渲染、与引擎同进程,自带虚拟化表格、Dock 面板、AccessKit 无障碍。
- **备选:Slint**,软件渲染兜底更好、输入法风险更低,但授权需走 Royalty-Free 并署名。
- **选型状态:待验证。** 必须在 M0 通过三项验证才能定案,见 `decisions.md`。
- GUI 与引擎通过 `rz-app` 的事件流通信;进度事件批量合并推送,避免高频刷新。

## 7. 平台集成

- **Windows**:Win11 新式右键菜单用 `IExplorerCommand`(COM,MSIX 签名),并兼容旧菜单;MSI 或 MSIX 安装。长路径使用 `\\?\` 前缀。
- **macOS**:Finder Sync 或 Action Extension;Quick Look 预览插件;签名和公证;拖到 Dock 图标压缩。
- **Linux**:Nautilus 用户级 Scripts 子菜单、Dolphin ServiceMenu、Nemo actions、`.desktop` MIME 关联;分发用 AppImage、Flatpak、deb / rpm。
- 文件名编码、路径分隔符、大小写敏感性统一在 `rz-platform` 处理。

## 8. 转换与恢复

- `rz-app::tasks::JobSpec::Transform`：共享队列调度、暂停/取消/进度、密码重试及恢复报告，GUI 通过应用层使用。
- `rz-engine::transform`：ZIP / tar / 7z 文件内容流式转换，保留文件名、目录及可用属性；路径、冲突名、压缩比、总量、深度和元数据预算检查。单文件流后端先测量大小再读取，测量受 `set_read_control` 的取消与大小限制约束；可选 RAR 仍沿用 unRAR 的逐条临时文件。修复逐条用匿名临时文件校验，失败条目不进入新包。输出在目标目录生成临时文件，整体校验通过后 `persist_noclobber` 发布。
- `rz-formats::recovery`：ZIP 正常目录逐条读取及本地头扫描；7z 按独立块恢复，某一块失败不阻止尝试后续块。格式层不写解压路径，也不决定界面。
- ZIP / 7z 分卷由引擎在同目录临时空间切分并校验；卷集完整发布前拒绝覆盖任一已有卷。ZIP 采用标准 `.z01`…`.zip` 磁盘字段，7z 使用连续 `.7z.001` 体积段。格式层在完整卷集可用时提供组合读取器。
- 收藏保存在 `Settings::favorites`；GUI 设置保存移入后台并串行执行，过期快照不覆盖较新状态。密码明文不进入设置，仍使用 `SecretStore` 与 `rz-services` 的系统钥匙串实现。

## 9. 常用位置与目录双视图

- `rz-platform::user_directories` 通过 `dirs` 读取平台标准目录（Windows Known Folders、Linux XDG、macOS 标准目录）。
- `rz-app::locations` 合并系统目录、MRU 和旧收藏，规范化实际路径去重，过滤隐藏入口。`Settings` 保存 `recent_locations`（最多 20）、`hidden_locations`、`folder_view`，从旧 `recent` 回填，只有成功打开才更新历史；全部本地存储，不发送到统计服务。
- GUI `locations` 模块在后台获取位置和元数据，以虚拟化列表或按行虚拟化网格呈现；两视图共享项目身份、排序和选择，页面重载使用代次校验，设置异步持久化。文件夹视图复用相同布局，GUI 不直接调用平台目录 API。

## 10. 官网与统计服务

- 独立目录 `web/` 使用 Next.js / TypeScript,部署在 Vercel;它不进入 Cargo workspace,也不被 Rust 核心依赖。
- `POST /api/v1/telemetry` 接收 `rz-app::stats::Report` 的固定 7 个字段,仅允许 `activate` 和 `heartbeat`;严格拒绝未知字段。
- 服务端用 HMAC-SHA-256 转换安装 UUID 后保存。Postgres 只保留摘要、首次 / 最近上报时间、最近环境信息和按日去重的活跃记录;不存原始 UUID、文件数据或请求 IP。
- `GET /api/health` 检查服务和数据库连通性。自动更新继续使用 GitHub Release 的签名 `update.json`,不复制到 Vercel。
- 本地 `next dev` 默认使用被 Git 忽略的 SQLite 文件;Vercel Preview / Production 使用 Neon Postgres。运行环境选择放在 `web/src/lib/db/`,SQLite 与 Postgres 分别维护 Drizzle schema 和迁移,细节见 `web/README.md`。
- Vercel build 先校验 `DATABASE_URL`、`DATABASE_URL_UNPOOLED`、`TELEMETRY_HASH_SECRET`,再幂等执行 Postgres migrations;失败时终止部署,避免线上 API 缺表。
