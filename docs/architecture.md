# 技术架构

## 1. 分层

```
┌────────────────────────────────────────────────────┐
│ 表现层  GUI │ CLI │ Shell 扩展(各平台原生)              │
├────────────────────────────────────────────────────┤
│ 应用服务层  rz-app:任务队列、事件总线、设置、密码库、更新、统计  │
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

依赖方向只能自上而下。下层不得依赖上层。GUI 和 CLI 只通过 `rz-app` 使用核心能力,不直接调用格式层。

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

1. **纯 Rust 原生,读写**:zip、tar 系、gz、xz、zst、bz2、lz4、7z。基于 `zip`、`tar`、`flate2`(zlib-rs 后端)、`zstd`、`liblzma` 或 `lzma-rs`、`sevenz-rust2` 等 crate。性能关键路径(7z 固实块、多线程 LZMA2)自己做调度,不直接套用单线程接口。
2. **绑定成熟 C 库,只读**:RAR 用 `unrar`(做成 Cargo feature),长尾格式用 libarchive。
3. **格式嗅探**:按魔数识别,不按扩展名;支持嵌套识别。

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
- **Linux**:Nautilus 扩展、Dolphin ServiceMenu、`.desktop` MIME 关联;分发用 AppImage、Flatpak、deb / rpm。
- 文件名编码、路径分隔符、大小写敏感性统一在 `rz-platform` 处理。

## 8. 转换与恢复

- `rz-app::tasks::JobSpec::Transform`：共享队列调度、暂停/取消/进度、密码重试及恢复报告，GUI 通过应用层使用。
- `rz-engine::transform`：ZIP / tar / 7z 文件内容流式转换，保留文件名、目录及可用属性；路径、冲突名、压缩比、总量、深度和元数据预算检查。单文件流后端先测量大小再读取，测量受 `set_read_control` 的取消与大小限制约束；可选 RAR 仍沿用 unRAR 的逐条临时文件。修复逐条用匿名临时文件校验，失败条目不进入新包。输出在目标目录生成临时文件，整体校验通过后 `persist_noclobber` 发布。
- `rz-formats::recovery`：ZIP 正常目录逐条读取及本地头扫描；7z 按独立块恢复，某一块失败不阻止尝试后续块。格式层不写解压路径，也不决定界面。
- 收藏保存在 `Settings::favorites`；GUI 设置保存移入后台并串行执行，过期快照不覆盖较新状态。密码明文不进入设置，仍使用 `SecretStore` 与 `rz-services` 的系统钥匙串实现。

## 9. 常用位置与目录双视图

- `rz-platform::user_directories` 通过 `dirs` 读取平台标准目录（Windows Known Folders、Linux XDG、macOS 标准目录）。
- `rz-app::locations` 合并系统目录、MRU 和旧收藏，规范化实际路径去重，过滤隐藏入口。`Settings` 保存 `recent_locations`（最多 20）、`hidden_locations`、`folder_view`，从旧 `recent` 回填，只有成功打开才更新历史；全部本地存储，不发送到统计服务。
- GUI `locations` 模块在后台获取位置和元数据，以虚拟化列表或按行虚拟化网格呈现；两视图共享项目身份、排序和选择，页面重载使用代次校验，设置异步持久化。文件夹视图复用相同布局，GUI 不直接调用平台目录 API。
