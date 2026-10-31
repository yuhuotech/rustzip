# 决策记录

记录已做的重要决策及理由。要推翻其中任何一条,先在这里新增一条并说明原因。

## D1 项目许可证:`MIT OR Apache-2.0`(已定)

- 选宽松许可证,因为它能直接使用的库最多、分发限制最少:Rust 压缩类 crate、libarchive(BSD)、zstd、lz4、BLAKE3 都可直接使用。
- 不选 GPL:会挡住 macOS App Store 等分发渠道,与 unRAR 的授权混合有争议;Apache-2.0 自带专利授权,对压缩算法这类领域更安全。
- 不拷贝 7-Zip 源码(LGPL,静态链接不适合),按格式规范重写。
- RAR 只读,做成独立 Cargo feature `rar`,官方构建附带 unRAR 授权声明,发行版可关闭。

## D2 RAR 只读(已定)

只做解压、浏览、测试,不做 RAR 压缩。

## D3 UI 框架:首选 GPUI Kit,待验证(暂定)

- 不选 Tauri:依赖系统 WebView,Linux 的 WebKitGTK 在渲染、性能、版本上不一致,与"极致性能和兼容性"冲突;内存和冷启动也偏高。
- 不选 Slint 作为首选:授权需走 Royalty-Free 并署名,或整体 GPLv3,与项目许可证有摩擦。保留为**备选**。
- 首选 **GPUI Kit**(`longbridge/gpui-kit`):Apache-2.0、纯 Rust、GPU 渲染、自带虚拟化表格、Dock、AccessKit。
- 风险:版本为 0.x、依赖固定的 GPUI 快照;官网和 README 未提中文输入法;Linux 与老显卡兼容性无数据;无 GPU 时能否回退 CPU 渲染不明。
- **M0 必须通过三项验证才能定案**,任何一项失败则退回 Slint:
  1. 三平台文本框中文输入(拼音、五笔、日文),含密码框和快捷键。
  2. 渲染 100 万行条目的压缩包列表,测滚动帧率和内存。
  3. Linux 虚拟机、无独显旧机器、Windows 10 上启动和渲染无花屏。
- 验证未完成前,UI 代码必须与 `rz-app` 解耦,保证能换框架。
- 进展:`gpui-kit 0.8.0` 已在 macOS(Apple Silicon,Xcode Metal)上编译通过,release 构建约 15 MB;验证程序 `apps/gpui-probe` 能启动并持续运行 8 秒无崩溃。清单和判定标准见 `gpui-probe.md`。
- 用户在 macOS 上的实测(未说明具体机型和版本):两个输入框正常;100 万行表格滚动稳定在 120 帧、非常顺滑。发现一个问题:**深色模式不会自动跟随系统**。原因是 GPUI Kit 不自动同步主题,应用必须在窗口创建时调用 `Theme::sync_system_appearance`,并用 `window.observe_window_appearance` 在系统切换时再调用一次。验证程序已按此修复,但修复后的效果还没有人在真机上确认。正式 GUI 必须照做。
- 仍未验证:Windows、Linux 上的输入法和渲染,以及无独显 / 虚拟机环境。这几项不通过就不能定案。

## D4 自动更新与安装统计(已定)

- 自动更新:签名更新包(Ed25519 / minisign),GitHub Releases 加 CDN,详见 `product.md`。
- 安装统计:仅匿名 UUID 加基础环境信息,首次启动告知并可关闭,详见 `product.md`。不建用户体系。

## D5 视觉风格(已定)

采用 `docs/ui/preview.html` 的风格:冷灰底、单一青绿强调色、等宽数字、明暗主题。详见 `ui-spec.md`。

## D6 产品名称:RustZIP(已定)

- 对外显示的产品名为 `RustZIP`,大小写固定(R、Z、I、P 大写)。
- 仓库目录、crate、包名等技术标识保持小写:`rustzip`、`rz-*`。CLI 命令仍为 `rz`。
- 界面标题、安装包名、关于页、文档中的产品名一律写 `RustZIP`。

## D7 M1 的格式与并行实现选择(已定)

- 7z:`sevenz-rust2`(Apache-2.0,纯 Rust)。写入走固实块加多线程 LZMA2,加密时同时加密文件头。
- xz:`liblzma`(静态链接 xz 源码,公有领域),用其多线程编码器。zstd 用 `zstd` 的多线程模式。
- bzip2:`bzip2` crate,单文件 / tar.bz2 按块并行压缩,内存不足时回退单线程;当前并行解码见 D39。
- 读取接口加入 `for_each`(单遍流式)和 `reopen`(独立读取器):tar / 7z / rar 这类顺序格式一次解码全部条目,zip 用多个读取器并行解压。
- ZIP 并行压缩:小文件跨条目并行压缩后按输入顺序合并(`merge_archive`);超过阈值的未加密 Deflate 条目按 D41 分块并行,Store / 加密 / 预算不足时顺序编码。线程与在途数据受内存预算约束,条目顺序不受线程数影响。
- 压缩炸弹:压缩比限制只对解压后超过 256 MiB 的条目生效,避免误伤小而高度重复的数据;总大小上限和"写入不超过声明大小"始终生效。
- RAR:通过 `rz-ffi` 中的 `unrar_sys` 调用 unRAR,放在默认关闭的 `rar` feature 里,只读。unRAR 是 C++ 代码,攻击面比其余纯 Rust 后端大,需要在文档中提示。
- 已知局限:每个压缩比检查依赖条目声明的压缩后大小,7z / tar 这类固实格式没有逐条目的压缩后大小,只能靠总大小上限和"不超过声明大小"兜底。

## D8 联网、钥匙串与发布(已定)

- 联网与系统钥匙串放在独立 crate `rz-services`(HTTP 用 `ureq` + rustls,钥匙串用 `keyring`),`rz-app` 只定义 trait 并用假实现测试;GUI 通过它接入,核心 crate 不碰网络。
- 更新源:GitHub Release 里的 `update.json`(`releases/latest/download/update.json`),安装包也放在同一个 Release。信任来自内置 Ed25519 公钥,不来自 GitHub 或 TLS;私钥只存在维护者本机和仓库 Secret `RZ_UPDATE_KEY`。
- 更新只做到"下载、验签、交给系统打开安装",不静默替换运行中的程序。
- 统计:只有设置了 `RZ_STATS_URL`(构建期或运行期)才会上报,且必须在用户看过首次启动提示之后。服务端自建,GitHub 不能接收上报。
- 记住密码默认关闭,开启后存进系统钥匙串,条目名只用压缩包文件名。
- 文件关联用"备选"优先级,不抢用户已选的默认程序。
- 品牌图标:石墨黑圆角方块加白色粗体 Z,Z 的斜线中间有一条青绿色的拉链缝,由代码绘制(`make_icons` 示例),不依赖设计软件。早先的"青绿方块加拉链"和带质感的盒子方案因饱和度高、显得廉价而放弃;青绿色只留在缝上,与界面强调色呼应。

## D9 左栏工具的数据安全与恢复边界

- 转换单遍流式处理，输出另存且默认拒绝覆盖；校验通过才发布新文件，不删除源包。
- 修复是尽力恢复可读条目，不伪造损坏内容。ZIP 支持中央目录损坏时扫描本地头；7z 受固实块和文件头可读性限制。恢复结果附带条目数和跳过原因，不将部分恢复描述成完整修复。
- 写入接口尚不支持符号链接：转换明确拒绝，修复报告跳过。ZIP 中央目录丢失时本地头没有链接类型元数据，无法识别的链接会按普通文件恢复，报告必须明确告知这种属性损失。
- 密码本延续 D8 的系统钥匙串后端，列表只展示标签，明文只在编辑时按需读取，异步操作失败必须反馈。

## D10 常用位置自动整理与双视图

- 用户确认常用位置由系统目录和成功打开的压缩包所在目录自动组成，沿用旧收藏作为补充；系统目录固定顺序，最近目录按使用顺序最多 20 个，路径去重。
- 目录单击选中、双击进入；移除只隐藏入口，下一次成功打开该目录中的压缩包可重新记录。列表和紧凑图标两种模式共用于位置总览及进入后的文件夹页，右上角切换并保存。
- 使用 `dirs 6`（MIT OR Apache-2.0，已存在于依赖树）通过 `rz-platform` 读取 Windows Known Folders、Linux XDG、macOS 标准目录，避免假定英文目录名或未重定向路径。目录读取、元数据和去重在后台执行。

## 待决事项

- 官方安装包渠道(是否上架 Microsoft Store、Mac App Store、Flathub)。
- 最低支持的系统版本最终线(随 UI 框架验证结果确认)。

## D11 发布路径数据保护（S1 稳定性工程确立）

来源：测试稳定性方案 S1 首批实施（2026-10-25），实现"新建压缩默认不覆盖既有目标"等契约。测试见 `crates/rz-engine/tests/io_failures.rs`、`security.rs`、`transform_failures.rs`，`crates/rz-app/tests/edit_failures.rs`、`settings_failures.rs`、`password_failures.rs`。

- **compress 默认拒绝覆盖既有输出**：开始前检查 + 结束用 no-replace 原子发布（`tempfile` 的 `persist_noclobber`），发布前被其他进程抢占时失败而不覆盖。GUI 本就在提交前拒绝同名文件；CLI `add` 行为随之从"覆盖"变为"报错"。
- **compress/transform 临时文件唯一命名**（`.{名}.rz-*.part`）：同目标并发运行互不共享/截断临时文件；残留可识别且不会被打进新包。
- **解压永不跟随目标内的符号链接**：写每个条目前逐级 lstat 父目录（不缓存，防止"刚创建的目录被换成链接"的 TOCTOU）；目标本身是链接一律拒绝（含 Windows junction 场景思路）；目录元数据只回写到仍是真实目录的路径。
- **Replace 模式解压失败保全旧文件**：已有目标先写同级临时文件，完成读取并在临时文件上设置时间戳/权限后原子改名替换；读取损坏、取消、写入或元数据失败时旧文件字节不变。
- **同包编辑跨进程串行化**：按归档路径摘要落到有界 OS advisory lock；等待锁时检查取消。暂存目录与新包文件名含 PID + 递增序号，跨进程竞争不会静默丢失更新。
- **并行 ZIP worker 有界退出**：producer 完成时设置原子关闭标记并关闭任务通道。空闲 worker 每 10ms 检查标记；队列耗尽后退出，再由作用域 join。错误或取消路径不能让 `TaskManager::drop` 永久等待。
- **设置保存**：进程内串行 + 每次保存独立临时名；短写、sync、publish 失败保留上次有效文件。基准（bench）错误一律非零退出且吞吐在内容校验通过后才记录。
- **Debug 输出脱敏**：`CreateOptions`/`ExtractOptions` 手写 Debug，密码显示为 `<redacted>`；任务/日志走这些类型即不泄密。

## D12 任务记录与全局提示

- 全局提示统一用 Toast(窗口顶部居中),不再占用状态栏;成功 4 秒、错误 5 秒自动消失。打开压缩包失败时,按错误类型映射为用户能懂的说明(不是压缩包 / 暂不支持的类型 / 已损坏 / 找不到 / 无权限 / 安全限制),不显示系统原始报错。
- 任务结束后只进"任务记录",不再留在底部面板。失败任务必须进记录,因为 Toast 很快消失。等待密码的失败任务不记录(输入密码后会作为新任务重试)。
- 记录写入配置目录的 `history.json`(与 `settings.json` 同目录),先写临时文件再改名,最多 200 条,文件损坏时改名为 `.bad` 并以空记录启动。升级覆盖安装不碰配置目录,因此记录保留。
- 隐私:记录只存文件名、类型、结果、一句话原因和时间,不存完整路径;原因文本中的用户主目录替换为 `~`。

## D13 同名不失败:保留两者并加序号

用户要求任何同名情况都不应让任务失败,而是自动生成 `名称 (1)`、`名称 (2)`。取代原先"遇到已存在就报错"的默认。

- 解压新增 `Overwrite::Rename` 并成为引擎、CLI 与设置的默认;设置里旧的"询问"并入它(旧文件里的 `ask` 仍可读取)。显式选择跳过或覆盖仍按原策略。
- 以**顶层条目**为单位处理:目标里已有同名的文件、文件夹或符号链接时,整个顶层条目改名为 `名称 (n)`,其下内容跟着移动,所以文件夹内容不会被拆散,也不会合并进已有目录。文件的序号放在扩展名前(`a (1).txt`、`data (1).tar.gz`),文件夹放在末尾。已有的符号链接不会被跟随,新内容落在它旁边。
- 保留例外:包内仅大小写不同的名字(APK)在不区分大小写的磁盘上仍提前报错并说明原因,因为改名会破坏资源引用,见 `apk-case-conflict.md`。
- 带密码的包第一次因缺密码或密码错误失败时,本次已创建的顶层条目会被清理,以便输入密码后的重试不会落成 `名称 (1)`。
- 新建压缩、转换、修复的输出文件名被占用时,GUI 自动换成带序号的名字,不再提示失败;引擎层仍拒绝覆盖已有文件(D11 不变),CLI 的 `add` 与 `convert` 仍由用户指定确切路径。
- 序号逻辑在 `rz-engine` 的 `naming` 模块,供引擎与 GUI 共用。

## D14 异步打开与常用位置刷新

- 同一窗口中最后发起的压缩包打开请求拥有焦点；较早请求即使更晚完成，也可以加入标签，但不得切走当前标签或弹出过期错误。重复打开仍在加载的同一路径复用该请求。
- 用户改点其他标签、菜单页或常用位置目录后，迟到的打开回调不再抢焦点。常用位置扫描只允许最新一代结果更新列表；旧扫描不能恢复已移除的位置或旧选择。

## D15 官网、统计接口与托管数据库

- 官网和统计接收服务放在仓库 `web/`,技术栈采用 Next.js / TypeScript,在 Vercel 部署;保持与 Rust workspace 隔离。
- 本决策更新 D8 中“统计服务自建”的部署方式:统计 API 随官网运行在 Vercel,数据库由 Neon 提供。
- 使用 Neon Postgres 的 Vercel 集成和数据库分支,兼顾 Vercel Serverless Function 的远程连接、本地开发与 Preview 隔离;Drizzle ORM 管理 schema 和迁移。
- 统计接口只兼容现有安装统计协议,不新增文件或操作行为采集。写库前用稳定服务端密钥 HMAC 安装 UUID,数据库不保存原始 UUID 和 IP。
- 自动更新仍以 GitHub Releases 的已签名清单为准,不迁入官网服务,避免改变既有更新信任链。

## D16 本地 SQLite 与 Vercel Neon 分环境

- 更新 D15 的本地数据库方案:本机 `next dev` 默认使用 SQLite 文件库,启动时自动应用 SQLite 迁移;不要求安装 PostgreSQL 或连接 Vercel 数据库。
- Vercel Preview / Production 继续使用 Neon Postgres,Preview 使用隔离分支;正式域名和服务变量按部署文档配置。
- 上报协议与去重规则由同一服务层实现,两种数据库各自保留 Drizzle schema 和迁移;本地 SQLite 数据不上传、不与线上同步。
- Vercel build 在 Next.js build 前校验 Neon URL 与统计 HMAC 密钥并幂等应用 Postgres migrations;缺少配置或迁移失败时阻止部署。

## D17 LZ4 使用标准 Frame 格式

- `.lz4` 与 `.tar.lz4` 使用 LZ4 Frame 格式，读写均采用流式接口；不接受仅凭扩展名推断内容。
- 使用纯 Rust `lz4_flex` 并保留默认安全编码、解码和 checked-decode 特性；许可证为 MIT。Frame 支持所需的 `twox-hash` 许可证也为 MIT，符合项目依赖许可要求。
- 单文件 `.lz4` 由核心和 CLI 创建；GUI 的多文件压缩与转换入口提供 `.tar.lz4`。

## D18 ZIP 与 7z 分卷命名和发布

- 第一版分卷只用于 ZIP 与 7z；调用方显式设置每卷大小，默认不分卷。ZIP 按标准 split archive 输出 `.z01`、`.z02`…及最后的 `.zip`；7z 输出 `.7z.001`、`.7z.002`…，读取均要求卷序连续且完整。
- 分卷按完整归档记录边界安排；ZIP 单条目数据允许跨卷，文件头和中央目录记录不跨卷。小于分卷阈值的归档仍输出普通单文件。
- 所有卷先写到目标目录内独占的临时文件并校验，再用拒绝覆盖的方式逐个发布；任一卷发布失败时只清理仍可确认为本次创建的卷。读取端只在完整卷集存在时开放归档。
- 分卷大小至少 64 KiB；ZIP 单卷最大 4 GiB - 1。达到格式字段上限时明确报错，不截断或静默改成单卷。
- ZIP 多磁盘字段按 [PKWARE APPNOTE 6.3.9，第 4.3 和 8.5 节](https://www.pkware.com/documents/casestudies/APPNOTE.TXT) 编写。Windows 发布回滚用 `same-file`（MIT OR Unlicense）核对目标仍指向本次暂存文件；Unix 使用 device / inode 标识，避免误删被其他进程替换的目标。

## D19 文件哈希使用流式 SHA-256 / BLAKE3

- 文件哈希由 `rz-app` 提供，单次 256 KiB 读取同时更新选择的算法；不将文件整体载入内存，并支持取消和进度回调。
- 默认计算 SHA-256 与 BLAKE3；CLI `rz h` 可通过 `-a` 选择一种。BLAKE3 依赖 `blake3`，许可证提供 Apache-2.0 选项，符合项目许可；不启用 rayon 和 mmap 特性，避免不受控的并行度或映射大文件。
- GUI 在最近文件和常用位置的普通文件操作中提供同一哈希任务；目录不作为输入。完成后显示算法和摘要，任务受统一队列管理。
- 归档完整性测试 `rz t` 继续验证归档后端提供的 CRC / 数据校验，不替代文件哈希命令。

## D20 7z 高级压缩参数与内存预算

- 7z 支持 LZMA2 字典大小和最大固实块大小；字典采用字节值，允许 4 KiB 至 64 MiB。自动模式沿用压缩级别对应的默认字典，预算不足时依次减半；显式值超过预算时返回资源限制错误。
- 固实块大小未设置时保持一个归档级固实块；设置正数时按最大未压缩字节数切块，0 表示每个文件独立编码。分块时同时限制每块最多 4096 个文件，控制单块元数据规模。
- LZMA2 并行线程、输入块缓存和估算编码工作集受 `CreateOptions::memory_budget` 限制；超过可用空间时降低线程数。CLI 使用 `-d size` 和 `-s size|off`，GUI 仅在选择 7z 时显示对应输入。

## D21 长尾格式使用隔离的 libarchive 读取器

- 首批通过 libarchive 只读支持 ISO 9660、CAB、CPIO 和 AR；DEB 容器按 AR 读取。libarchive 的格式注册只开启上述四种后端，不开启其 RAR、ZIP、7z 支持；只注册无压缩滤镜，避免解压器调用外部命令。
- 原生调用封装在 `rz-ffi`，其余核心保持 `forbid(unsafe_code)`。`libarchive2-sys` 和 vendored libarchive 均采用 BSD-2-Clause；静态构建需 CMake、Clang/libclang 及平台压缩库开发文件。
- libarchive 读取器标记为顺序访问、只读；不保留符号链接和特殊文件，统一作为跳过项交给现有安全解压策略。未验收格式不因 libarchive 可能支持而自动开放。

## D22 CLI 通过应用层执行且不接收明文密码参数

- CLI 的创建、列表、解压、完整性读取、转换和修复通过 `rz-app::cli`；CLI crate 不直接依赖 `rz-engine` 或 `rz-formats`。长操作使用取消 token 和进度接口。
- `-p -` 使用隐藏 TTY 提示，`-p @file` 读取密码文件的第一行且上限 4 KiB；不再接受明文密码作为参数。命令行使用 `ctrlc`（MIT OR Apache-2.0）处理 Ctrl+C，密码读取使用 `rpassword`（Apache-2.0）。
- CLI 列表使用 `ArchiveReader::entries_with_progress`；ZIP、7z、tar、RAR、libarchive 和单文件流后端在枚举条目时检查取消并报告进度。单文件流计算解压大小时也可以取消。

## D23 桌面任务通知与完成后定位

- 解压成功后可按设置在文件管理器中定位结果目录；系统通知作为单独开关，默认关闭。通知仅显示通用成功 / 失败状态，不传文件名、路径或文件内容。
- 通知服务使用 `notify-rust`（MIT OR Apache-2.0），通过后台 executor 调用；通知失败不改变任务结果，也不阻塞 GUI。关机等高影响动作不包含在本决策中。

## D24 邮件分享与跨平台兼容预设

- 邮件分享预设仅用于 ZIP / 7z，默认将最大卷大小设为 20 MiB；界面已填写的分卷值优先于预设值，空值才使用该默认值。
- 跨平台兼容预设保持均衡压缩级别并设置 `CreateOptions::portable`，不保留源权限；7z 省略 Windows 文件属性，ZIP 和 tar 使用格式默认权限。其他格式不会显示邮件分享选项。

## D25 压缩前重复文件检查

- GUI 压缩前递归检查最多 100,000 个普通文件，忽略符号链接和特殊文件；先按大小分组，只对候选组做流式 BLAKE3 比较。检查任务使用统一队列，支持取消和进度。
- 发现重复时按内容摘要提示组数、文件名和多余字节数；用户可取消或继续压缩全部输入。首版不自动删除文件，也不更改 CLI 和转换行为。

## D26 CLI 转换与修复命令

- `rz c` / `rz convert` 将归档流式转换为 ZIP、7z 或 tar 系列；`rz r` / `rz repair` 尝试从 ZIP / 7z 恢复可读条目并报告跳过项。
- `-p` 提供输入包密码，`-q` 提供输出包密码；两者只接受隐藏终端提示或 `@file` 首行，不接受明文参数。源与目标密码分别输入，匹配 GUI 的独立密码边界。
- 转换和修复的目标路径必须是新文件；首版不支持分卷输出。两种操作通过 `rz-app::cli` 执行，并支持 Ctrl+C 取消和进度回调。

## D27 界面语言选择

- 设置提供 `auto` 和十种界面语言：简体中文、英语、西班牙语、法语、德语、日语、韩语、阿拉伯语、葡萄牙语、俄语。选择面向常见桌面软件用户的覆盖，而不是直接照搬总说话人数排名；界面语言标签使用各语言自称。
- 显式选择立即保存并刷新窗口、表格、右键菜单和原生菜单。`auto` 使用 `sys-locale` 读取操作系统的首选语言；macOS 读取用户首选语言列表，Windows 读取用户 UI 首选语言，Unix 使用 `LANGUAGE` / `LC_ALL` / `LC_MESSAGES` / `LANG`。按顺序选择 RustZIP 支持的第一种语言；没有匹配项或读取失败时使用简体中文。
- 简中和英文使用共享资源表，其余八种语言使用 `apps/gui/resources/locales/` 独立词条包。覆盖测试要求每种语言包含全部资源键，并保留 `{placeholder}`；缺项会回退到英文。
- 系统 API 的 Linux Ubuntu 26.04 实测和 macOS `zh-Hans-CN` 偏好映射通过；Windows 原生语言选择和跨平台辅助技术仍待设备验收。

## D28 文件列表键盘焦点反馈

- 文件表恢复 GPUI DataTable 默认的键盘焦点环。组件仅在键盘输入导致焦点落在列表时显示焦点环，鼠标点击和 Ctrl+A 不会无条件闪出焦点环。
- 文件表容器提供可读名称；无文字按钮、主题色 swatch、压缩/密码表单输入和任务进度控件提供显式辅助标签。主题色 swatch 可通过 Tab 聚焦并以 Enter / Space 激活。完整屏幕阅读器验收仍需跨平台进行。

## D29 任务记录的结果定位

- 成功任务在本次运行内由任务摘要提供结果路径；任务记录页可通过“显示结果”调用系统文件管理器定位结果。覆盖压缩、解压、转换、修复、编辑、哈希和完整性测试。
- 结果路径仅留在当前进程的任务摘要中，不写入 `history.json`；重启后保留记录文本，但不显示定位操作。任务被用户清除后相应定位入口也消失。

## D30 PDF 内置预览的资源安全边界

- PDF 内容与归档内容一样是不可信输入。内置 PDF 渲染必须限制解压后数据量和栅格像素数；只限制源 PDF 文件大小与最终位图大小不足以防止压缩数据放大耗尽内存。
- 当前评估的纯 Rust Hayro renderer 使用 Apache-2.0 OR MIT，且禁止 unsafe，但默认 Flate fallback 将解码输出不断追加到 `Vec`，没有输出上限；可选的标准解码路径也调用无上限的 `read_to_end`。仅限制源文件字节数和最终像素数不足以防止解压炸弹，所以在渲染器实现可执行的解压输出硬上限前不接入。依据：[Hayro 渲染说明与许可证](https://github.com/LaurenzV/hayro)、[Flate 解码实现](https://github.com/LaurenzV/hayro/blob/main/hayro-syntax/src/filter/lzw_flate.rs)。
- `pdfium-render` 的 Rust 绑定许可证为 MIT OR Apache-2.0，但不包含 PDFium 动态库，需额外下载、构建和跨平台打包原生二进制；因此本轮不选用。PDFium 的归档样本、运行时限制和包体成本需要单独评估。
- 2026-10-29 复查 MIT OR Apache-2.0 的 `pdfrum` 0.5.3：提供单 PDF stream 解码字节上限、每次 render 像素上限和可协作 deadline，且默认不启用 JavaScript；但已公开的上限针对单 stream，无法配置一页所有图片 / 内容 stream 的累计解码字节预算，deadline 也在解析 / page / 操作符 / render 对象等检查点协作执行，不能中断任意长的单个底层解码调用。因此仍未证明页面总解码内存有硬上限，不接入。依据：[Limits API](https://docs.rs/pdfrum/latest/pdfrum/struct.Limits.html)、[OpenOptions](https://docs.rs/pdfrum/latest/pdfrum/struct.OpenOptions.html)、[项目主页与许可证](https://github.com/PoHsuanLai/pdfrum)。
- 2026-10-30 复查 `pdf-engine` 1.0.0：虽提供单 stream、单文档总内存和图像像素上限，但许可证为 `AGPL-3.0-only OR LicenseRef-PDFluent-Commercial`，不符合 RustZIP 依赖许可策略，故不接入。依据：[crate 版本与许可证](https://docs.rs/crate/pdf-engine/1.0.0)、[ProcessingLimits](https://docs.rs/pdf-engine/1.0.0/pdf_engine/limits/struct.ProcessingLimits.html)。
- Hayro 当前项目说明仍列出不可信 PDF 可触发 OOM / timeout，且上游仍有交互渲染协作取消需求；PDFium 的 limited-image-cache 是降低内存使用的选项，不是可配置的累计解码输出硬上限。依据：[Hayro robustness status](https://github.com/LaurenzV/hayro)、[cancellation issue](https://github.com/LaurenzV/hayro/issues/1052)、[PDFium render cache option](https://docs.rs/pdfium-render/latest/pdfium_render/struct.PdfRenderConfig.html)。在累计解码上限或 OS 隔离限制可验证前，预览继续调用系统 PDF 阅读器。
- 达到资源边界后，内置预览需清楚提示用户改用系统 PDF 阅读器；在满足上述限制之前继续由系统程序打开 PDF。

## D31 LHA / LZH 只读后端

- 在 D21 隔离的 libarchive reader 中新增 LHA/LZH 注册；只注册 LHA reader，不注册 7z、ZIP、RAR 或外部命令滤镜。
- 格式嗅探检查头部方法字段 `-lh?- / -lz?- `，之后仍由 libarchive 验证实际结构。LHA 只读，链接条目仍交由通用安全策略跳过。
- 回归 fixture 取自上游 libarchive 的 BSD-2-Clause LHA `-lh0-` 测试数据，覆盖目录、相对链接和普通文件；来源与内容记录在 `crates/rz-formats/tests/fixtures/README.md`。

## D32 XAR 只读后端

- 在 D21 隔离的 libarchive reader 中新增 XAR 注册；只注册 XAR reader，不注册 7z、ZIP、RAR 或外部命令滤镜。
- 通过 `xar!` 文件头识别格式，XAR 归档内部压缩由其格式 reader 解码；条目仍经通用路径和特殊文件安全策略处理。
- 测试 fixture 由 bsdtar 3.7.4 生成，只含虚构文本文件；不引入新的依赖或测试数据许可证。

## D33 界面语言快捷入口与窗口顶栏位置

- 设置按钮始终放在应用内容区最上方的标题栏或标签栏。打开压缩包后，设置按钮随标签移动到标签栏右侧；不再放在归档操作工具栏。
- 语言菜单按钮紧邻设置按钮，菜单包含“跟随系统”和全部受支持语言；当前值使用菜单选中态表达。按钮提供可读名称，菜单可用鼠标和键盘操作。
- 欢迎页和归档页使用相同的右侧控制顺序：语言、设置。Windows/Linux 的原生窗口标题栏保持由系统绘制，两个控件位于窗口内容区首行。
- 固定宽度侧栏里的导航文字单行显示，侧栏资源可使用简短译法；空间不足时以省略号裁切，完整条目名仍用于辅助名称。

## D34 RPM 使用内置 filter 与 CPIO 只读

- 在 D21 隔离的 libarchive reader 中新增 RPM 支持。RPM 魔数、版本和包类型字段通过验证后，固定注册 RPM filter、gzip/bzip2/xz/zstd 内置解码器和 CPIO reader；不注册 program filter，也不调用 `rpm2cpio` 等外部命令。
- RustZIP 仅列出、测试和提取包内普通文件；不执行包安装脚本，不保留链接等特殊文件，不提供 RPM 写入。
- sniff 使用 RPM 包头魔数与版本 / 类型字段，不以 `.rpm` 扩展名判定。bzip2 和 gzip 测试 fixture 来自 vendored libarchive 的 BSD-2-Clause 测试数据，由三份虚构的 `hello` 文本文件组成；没有新增 Cargo 依赖。

## D35 bzip2 并行压缩与初始 RAR 预览限制

- 更新 D7 的 bzip2 单线程状态：单文件和 tar.bz2 可将输入分块并行压缩为按顺序拼接的标准 bzip2 流；线程数受内存预算估算限制，不足时回退单线程。D39 后续增加并行解码。
- 本决策当时的 unrar 后端在提供条目流之前会把整个条目写入临时文件，不能保证预览磁盘上限，故先禁用 RAR 预览。该实现状态已被 D38 的逐块回调和硬输出上限取代；D35 仅保留历史原因。

## D36 安装包的 RAR 能力与关联一致性

- D1 规划正式发行版附带 RAR，但默认构建仍关闭该 feature。D38 已为 RAR 解码增加逐块输出上限和取消；当前 HEAD 的 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 已构建并检查四平台 opt-in RAR 测试包、随包 unRAR 许可及 Windows 安装 / 关联 / 卸载。正式签名发布仍待验收。
- 桌面应用暴露默认关闭的 `rar` feature；本地打包默认不启用 RAR，也不注册 RAR 专属文件关联。显式启用 RAR 的本地测试包可同时注册相应关联，并继续遵守 unRAR 许可证。三平台的设置方式在发布文档中说明。
- CI [40939090908](https://github.com/yuhuotech/rustzip/actions/runs/40939090908) 在 Windows、macOS、Ubuntu/Xvfb 运行 all-features GUI Clippy / 测试并通过；当前 HEAD 的 Release dry-run [41012225195](https://github.com/yuhuotech/rustzip/actions/runs/41012225195) 随后验证了四平台 RAR 包许可证与 Windows 实际安装 / 卸载关联。默认发行构建仍关闭 RAR。

## D37 长尾格式后端准入

- CHM 以只读格式接入 `rz-formats::chm_fmt`，使用 MIT `libchm` 0.3.0 的固定 vendor 副本。局部补丁提供范围读取、directory 回调和 LZX block 取消；条目读取每次最多 64 KiB、每次操作总输出按应用设置限制（未设置时为 1 TiB）、单压缩 block 4 MiB、reset table 8 MiB、目录元数据 16 MiB / 100,000 条。测试 fixture 由独立 `rustchm` 0.1.2 生成；解压路径仍统一经过引擎的 `safe_join`。
- `fuzz/fuzz_targets/chm.rs` 会模糊测试 CHM 打开、枚举和范围读取。2026-10-29 本机 nightly ASan fuzz 已运行 15 秒，约 21,000 次变异未发现崩溃；这只作为短时冒烟，发布前仍需在持续 nightly runner 中重复运行。
- WIM 和 DMG 仅按魔数 / trailer 识别并返回明确的“不支持”。已审候选只读镜像或分区元数据，或缺少对元数据、解压和取消的完整控制；WIM 完整候选还涉及 LGPL / GPL 静态链接许可及更高 MSRV。2026-10-30 复查 `wim-format` 0.1.1 确认其许可证为 LGPL-2.1-or-later OR GPL-3.0-or-later；MIT `caddy-archive-core` 的 WIM feature 仍依赖该 crate，不能绕过限制。NSIS 仅按 D47 的 bounded FirstHeader 规则识别，读取仍不支持。
- 最新 `nsis` 0.6.0 为 Apache-2.0，增加了分块输出预算，但公开入口仍接收整份 `&[u8]`，solid 文件流仍被解压到缓存 `Vec`；没有 caller-driven cancellation / streaming entry reader，暂不注册。依据：[nsis crate API](https://docs.rs/nsis/0.6.0/nsis/installer/struct.NsisInstaller.html)、[solid stream behavior](https://docs.rs/nsis/0.6.0/nsis/installer/enum.SolidStatus.html)、[`wim-format` 许可证与 API](https://docs.rs/crate/wim-format/0.1.1)。
- 新格式 reader 只有在提供流式 / 有界读取、分块取消与进度、元数据和解压输出硬上限、通用路径安全处理、损坏输入与 fuzz 覆盖，以及实际工具生成的独立 fixture 后，才可注册并宣称支持。具体候选证据见[第三阶段记录](phase-3-format-assessment.md)。

## D38 RAR 解码输出上限与安全预览

- 保留默认关闭的 `rar` feature 和只读边界；将 UnRAR 的逐块数据回调封装在 `rz-ffi`，解码前检查压缩比，并在回调中检查取消、声明大小和本次选中输出的剩余预算，超限时立即中止原生解码。
- 选中条目逐块写入私有临时文件，单个文件不超过其声明大小和剩余 `max_total` 的较小值；每项消费后删除。未选条目通过 UnRAR 测试流继续固实解码，不写临时盘，并在数据回调中检查取消。
- `ArchiveReader::for_each_selected` 让引擎将选择索引交给顺序格式；RAR 可以跳过未选条目的临时输出，同时保留固实流状态。兼容性 `open` 的整条目内存读取限制为 64 MiB。`open` 对选中条目直接使用有界内存缓冲，不落临时盘；通用选择性提取仍逐块写入私有临时文件，避免大条目整体驻留内存。
- 图片预览使用 8 MiB 上限、文本预览使用 64 KiB 上限；RAR 只在启用 `rar` feature 的构建中可预览。该决策取代 D35 的 RAR 预览禁用状态及 D36 中“临时输出没有硬上限”的旧实现状态，不改变 D36 的默认关闭、许可证和文件关联要求。
- 回归覆盖原生回调超限中止、取消、RAR 测试、选择性输出、多个选中条目的累计总输出限额及应用层图片 / 文本读取限额。三平台 CI [41097739228](https://github.com/yuhuotech/rustzip/actions/runs/41097739228) 的 core default / all-features 测试均通过该累计上限用例。Release dry-run [41090713832](https://github.com/yuhuotech/rustzip/actions/runs/41090713832) 已验证四平台 opt-in 测试包、许可证和 Windows RAR 安装 / 卸载关联；正式签名 Release 仍待验收。
- 当前 HEAD `db0d14e` 的 GitHub Fuzz run [41005418831](https://github.com/yuhuotech/rustzip/actions/runs/41005418831) 对 RAR target 执行 233,347 次变异并保存 corpus cache，未发现崩溃。

## D39 bzip2 并行解压与 gzip 分块 Deflate

- 使用 MIT `pbz2` 0.2.0 及其 MIT `pbz2-core` 做 bzip2 顺序流和串接 member 的并行解码；保留上游 `NOTICE` 中的 bzip2 / libdivsufsort 归属与许可说明。wrapper 固定为本地 vendor 副本，改动限于 decoder worker 启动错误返回和后台线程异常传递。
- bzip2 解码的 worker 数按 `min(requested_threads, floor((memory_budget - 16 MiB) / 64 MiB))` 估算；不足两个 worker 时回退现有 `MultiBzDecoder`。压缩流按块 CRC 与 stream CRC 校验，未消费完即 drop 可停止后台工作。
- gzip 与 tar.gz 在多线程模式下把输入分成最多 900,000 字节块，按顺序生成标准串接 gzip member；单线程或预算不足时沿用 flate2 单 member 编码。`MultiGzDecoder` 和常见 gzip 工具可读取。
- 初始实现的分块 Deflate 覆盖 `.gz` / `.tar.gz`；ZIP 大条目扩展实现与验证见 D41。

## D40 ZIP 依赖安全版本

- 工作区 ZIP crates 统一使用 `zip` 9.1.3，修复 RustSec `RUSTSEC-2026-0367/0368/0369` 覆盖的权限恢复和符号链接逃逸问题；9.x 后续发行也包含提取路径与 ZIP64 处理修正。升级移除了由 2.6.1 带入、命中 `RUSTSEC-2026-0391/0392/0398/0403` 的 zstd 0.13 依赖。
- RustZIP 的解压仍由 `rz-engine` 的 `safe_join`、不跟随符号链接写入和权限规范化实现；不得调用 `ZipArchive::extract` 绕过引擎策略。ZIP 大条目并行 Deflate 已按 D41 接入，仍需 Windows / macOS 外部工具互通验收。
- 升级后的 `zip_recovery` nightly ASan fuzz 冒烟运行约 91,000 次变异，未发现崩溃；ZIP 编辑 8 MiB raw-copy / repack 基准均按输出 SHA-256 校验通过。更大样本和多轮采样仍待执行。
- 锁文件升级必须运行工作区 all-features 测试和 `cargo deny check`；[deny.toml](../deny.toml) 限定受支持的平台目标和许可白名单，不允许 GPL / LGPL。

## D41 ZIP 大条目分块 Deflate

- 多线程创建 ZIP 时，小文件继续跨条目并行；超过现有条目阈值的大文件在未加密 Deflate 模式下按 900,000 字节切块，由有界 worker 压缩并按输入顺序写入。各块在 Deflate 边界处刷新后接成一个标准 Deflate stream，外层 ZIP 仍由 `zip` crate 写入和校验。
- worker 数按 `min(requested_threads, floor(memory_budget / 16 MiB))` 估算；最多保留每个 worker 两个块的输入 / 压缩结果。输入使用流式读取；压缩中间数据写匿名临时文件，不在内存中组装整条目。该预算用于约束并行工作集，不代表操作系统级内存限制。
- ZIP 加密条目继续使用现有顺序 writer；级别为 Store、仅一个 worker 或预算不足时回退顺序路径。源读取检查现有任务取消 / 暂停逻辑，读错或取消时 worker 在关闭通道后退出，外层临时归档由引擎清理。
- 本机验证覆盖多块大条目往返、CJK 文件名、源中途失败、Info-ZIP `unzip -t` 和 BLAKE3 校验基准。Windows / macOS 工具互通与三平台性能矩阵仍需验收。

## D42 ZIP 压缩数据复用时保留容器元数据

- 符合复制快路径条件的单卷、无密码 ZIP 编辑保留归档注释、ZIP64 扩展数据、未修改条目的文件注释和扩展字段；替换条目时保留归档注释与该条目的文件注释。重建中央目录时将原始扩展字段交由 `zip` 9.1.3 的 raw-copy API 复制。
- 快路径只接受 ZIP 起始偏移为 0 的归档；带自解压前缀、非 UTF-8 原始文件名或不满足路径 / 资源限制的归档回退完整解压重打包。编辑仍写入同目录新文件并校验后原子替换，遵守 D11；这不实现原地增量写入。
- 回归用例检查删除与重命名后归档注释、保留条目的文件注释 / 自定义 extra field，以及替换后的归档和条目注释。
- staged append / reflink 路径扩展见 D53；本决策描述的 raw-copy 重建路径继续作为文件系统不支持 reflink 或 append 不适用时的回退。

## D43 图片预览像素上限

- 归档图片即使压缩文件小于现有 8 MiB 预览字节上限，也可能声明极大的像素尺寸并诱发高内存解码。GUI 在提交 GPUI 图片解码前先读取受支持格式的图像头；尺寸检查在后台执行。
- 详情栏最多接受 16,777,216 像素（RGBA 解码约 64 MiB），且保留既有图片格式白名单与 8 MiB 压缩字节限制。未知、损坏或超像素上限的图片使用文件类型卡片，不尝试解码。
- 该限制只影响详情栏预览；用户仍可显式将条目提取到磁盘并用系统图片程序打开。
- `apps/gui/src/image_preview.rs` 的回归用例覆盖刚好达到上限、超过上限、零尺寸和损坏图像头；GUI workspace 测试与 Clippy 通过。

## D44 压缩包对比边界

- 比较归档内的规范化相对路径与条目类型；普通文件通过 256 KiB 缓冲流式计算 BLAKE3，只对同路径、同大小文件计算双方内容。不同大小直接报告变化；时间戳和压缩后大小不参与内容判定。
- 显式目录和文件路径隐含的父目录视为同一目录。重复文件路径和文件 / 父目录冲突返回损坏错误，避免生成含糊差异。
- 遵守调用方压缩比与总解压大小设置、任务取消和进度回调；每个归档最多接受 1,000,000 个枚举条目。比较不写临时文件或提取数据。
- `ArchiveReader` 不暴露符号链接目标，因此匹配的符号链接结果必须标记为“未比较”，不能报告为内容相同。
- GUI 仅从已打开的压缩包标签中选取另一包；CLI `rz diff` 接受共用密码 `-p` 或独立的 `--left-password` / `--right-password`。任务历史只保存比较类型、文件名和汇总，不持久化差异清单或完整路径。
- 2026-10-30 回归覆盖加密 7z 的左右独立密码、RAR、CHM、ISO 9660、LHA/LZH、RPM、XAR，以及相同内容的跨格式比较；定向 app 和 CLI 测试通过。该 fixture 覆盖不替代大型真实归档性能采样或跨平台验收。

## D45 macOS Finder 右键菜单

- macOS `.app` 嵌入 Finder Sync 扩展，在 Finder 对已选项目的右键菜单中提供 RustZIP 压缩；所有所选条目扩展名都属于已支持归档时，也显示“解压到此处”。RAR 入口只出现在启用 `rar` feature 的构建。
- 扩展不读取归档或源文件内容；它将所选本地 `file://` URL 经 `rustzip://compress` 或 `rustzip://extract-here` 传给宿主应用。GUI 复用 `Launch` 和现有应用层流程。解析器仅接受本机路径、检查路径存在类型，并限制 URL 长度和条目数。
- Finder Sync 扩展使用 App Sandbox 和只读用户所选文件权限，不申请共享容器；菜单文案随扩展 bundle 提供十种受支持语言。
- 动作 URL 由已有锁定版本的 `url` 2.5.8 解析（MIT OR Apache-2.0）；它原本已是工作区传递依赖，此次仅在 GUI 中显式声明。URL 总长限制 1 MiB，单次动作最多 4,096 个路径。
- Finder Sync 扩展由用户在 macOS 系统设置中启用。该设置授权扩展读取 Finder 所选项目 URL；RustZIP 默认桌面构建仍保持未沙箱化，以继续使用现有系统钥匙串与文件访问流程。
- 2026-10-30 在 macOS 27.0.1 的本机测试 bundle 中手动点击压缩和解压菜单：压缩打开 RustZIP 创建窗口；解压后文件 BLAKE3 与测试 ZIP 内文件一致。该 ad-hoc 测试不替代 Developer ID 签名安装包、公证后启用验收。

## D46 macOS Quick Look ZIP 目录预览

- `.zip` Quick Look 首版使用 `NSViewController` 视图式扩展，只读取中央目录并展示路径、条目类型、解压后大小和加密标记；不解压文件内容、不渲染归档 HTML / 脚本，也不生成缩略图。普通图片 / 文本的应用内预览仍由 RustZIP 详情栏提供。
- ZIP 预览最多读取 16 MiB 中央目录、解析 50,000 个条目并展示前 200 项；支持 ZIP64 目录元数据。分卷 / 多磁盘 ZIP 显示明确的不支持提示。损坏目录或超过限制时显示安全提示。
- Quick Look 扩展使用 App Sandbox 和只读用户所选文件权限，按系统请求的 URL 读取。条目名必须经 HTML 转义；不得使用网络资源、脚本或解压内容。
- `.7z`、RAR、tar 系和其他类型暂不注册 Quick Look provider，避免在应用层之外重复引入未验证的解析后端。
- `packaging/macos/test-quicklook.sh` 的 Swift smoke 覆盖常规 ZIP、ZIP64 EOCD / 越界长度、CJK / 特殊字符名称、HTML 转义、损坏中央目录、多卷输入和 16 MiB 上限。macOS 27.0.1 上 `qlmanage -p -x` 曾启动 Quick Look 扩展并确认 WebKit HTML 页面加载。2026-10-30 使用临时包验证：先经 Launch Services 注册父 `.app`，再显式注册 `.appex` 并设置 PlugInKit election 为 use 后，provider 出现在 `com.apple.quicklook.preview` 列表，`qlmanage` 启动了 `RustZIPQuickLook` 进程。该次 Finder 空格键可见 UI 因桌面捕获不可用而未验收；临时 PlugInKit 和 Launch Services 注册已撤销。正式签名 / 公证包仍需完成 Finder 可见界面与注册验收。
- `.github/workflows/release.yml` 的 macOS dry-run / release matrix 运行 `packaging/macos/verify-quicklook-registration.sh`：先注册父 app，再将 provider 设为 PlugInKit `use`，确认 provider identifier、extension point 和实际 appex 路径均匹配，退出时恢复默认 election 并移除临时注册。macOS 27.0.1 本机及 Release dry-run [41043955928](https://github.com/yuhuotech/rustzip/actions/runs/41043955928) 的 arm64 / x86_64 runner 均通过该 smoke；Finder Spacebar 可见页面仍需 GUI 实测。

## D47 NSIS 内容识别边界

- NSIS 仅在输入包含结构有效的 PE section table，并在 overlay 前 8 MiB 内找到标准或 NSIS 1.x FirstHeader 签名、合法 flags 和正数长度字段时识别。不得依据 `.exe` 扩展名、PE 签名或 `.ndata` section 单独认定为 NSIS。
- 探测只进行范围读取，section 数最多检查 96 个，FirstHeader 固定 28 字节；文件声明长度不会驱动分配或长距离读取。普通 PE 和不完整 / 不一致的 FirstHeader 返回未知格式。
- 此决策只增加内容识别和明确“不支持读取”的分流，不注册浏览、测试或提取 reader。reader 仍须满足 D37 的许可、流式输入、取消和硬资源上限要求后才能接入。
- GUI 对识别出的 NSIS 返回专用本地化提示；`.nsis` 之类扩展名本身不会触发该提示。
- `sniff_nsis` 由 synthetic PE overlay 单元用例和 nightly `nsis` fuzz target 覆盖；种子只包含虚构头部与零填充，不包含可执行代码。本机 nightly ASan 运行 119,304 次、GitHub fuzz run [41010338879](https://github.com/yuhuotech/rustzip/actions/runs/41010338879) 运行 3,287,482 次，均无崩溃并保存了 corpus cache。
- FirstHeader 的签名字段依据 [NSIS 官方 `fileform.h`](https://github.com/NSIS-Dev/NSIS/blob/master/Source/exehead/fileform.h)；识别代码不依赖第三方解析器或新增依赖。
- 另以 [上游 `deflate_nonsolid.exe` fixture](https://github.com/ATRAPSLLC/nsis-rs/blob/f20d03c407674dd670787ebcb2fa9e5f0d65a0ad/tests/fixtures/deflate_nonsolid.exe) 做本机只读冒烟：文件头确认为 NSIS，`rz l` 返回 NSIS 专属“不支持读取”错误。该 fixture 仅临时下载到 `/tmp`，不进入 RustZIP 分发内容。

## D48 密码本候选自动尝试

- 自动尝试继续受 D8 的“记住密码”开关控制，默认关闭。开启后，归档打开或解压 / 测试 / 编辑 / 转换 / 修复任务因缺密码失败时，先尝试以归档文件名为标签的已保存密码，再按密码本索引逆序检查其他候选；总数最多 16 个不同密码。
- 候选密码只从用户主动保存的系统钥匙串 / 密码本读取；每个候选仅验证一个最小的加密普通文件，最大读取 64 MiB。7z reader 不提供逐条目加密标记，因此只对一个 64 MiB 内的普通文件做有界试读；没有可验证的文件时不声称找到密码。所有失败后仍要求用户手动输入。
- 搜索在后台执行：未完成的归档打开搜索会随导航取消；任务搜索可从任务行停止，关闭“记住密码”或清空密码本也会取消。候选内容使用 `Zeroizing`，不写日志。密码本读取失败需提示钥匙串错误，同时保留手动输入路径。
- 此实现补齐 `product.md` 中“常用密码自动尝试”要求，不改变 D8 的归档文件名标签保存规则、默认关闭和系统钥匙串存储。

## D49 PDF 预览的 macOS 资源上限验收门槛

- 评估候选 `hayro` 0.7.1（Apache-2.0 OR MIT，MSRV 1.92）能够渲染第一页 PNG，但上游仍标记为实验性、持续开发中。依据：[上游项目与许可](https://github.com/LaurenzV/hayro)、[0.7.1 crate manifest](https://docs.rs/crate/hayro/0.7.1/source/Cargo.toml)。
- 2026-10-30 在 macOS 27.0.1 子进程中实际调用 `setrlimit`，`RLIMIT_AS`、`RLIMIT_RSS` 和 `RLIMIT_DATA` 均返回 `EINVAL`；本机 `man 2 setrlimit` 未列出可用的地址空间上限。XNU 当前头文件把 `RLIMIT_RSS` 定义为 `RLIMIT_AS` 别名，但这没有改变本机 syscall 的拒绝结果。依据：[Apple setrlimit 手册](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/setrlimit.2.html)、[XNU resource.h](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/resource.h)。
- Windows Job Object 和 Linux `RLIMIT_AS` 可提供进程内存限制，但仅有两个平台可用不满足 RustZIP 的跨平台预览承诺。父进程轮询 RSS 也不能替代硬上限；在 macOS 有可验证的硬限制或 renderer 自身具有累计解码预算前，PDF 继续由系统程序打开。
- 本次评估未把 `hayro` 或进程限制依赖加入产品，也未注册 PDF renderer。加密 PDF、坏 xref、嵌套炸弹和像素上限仍需同时满足 D30 后再实现。

本门槛的后续实现方案见 [D50](#d50-pdf-预览的分配预算与隔离-helper)。

## D50 PDF 预览的分配预算与隔离 helper

- 2026-10-31 接入 `hayro 0.7.1`（Apache-2.0 OR MIT，MSRV 1.92），但不在 GUI 进程内解析 PDF。GUI 将最多 16 MiB 的 PDF 内容通过标准输入传给单次启动的 `rz-pdf-renderer` 子进程；无文件路径参数，不写临时文件。
- 子进程的全局分配器限制同时存活的 Rust heap 请求总量为 256 MiB，累计分配总量为 512 MiB。分配失败只会终止隔离 helper；helper 自身和 GUI 父进程均设 10 秒时限，并支持由选择切换等界面操作取消。
- 每页最多输出 16,777,216 像素，单边不超过 8,192 像素，PNG 输出最多 8 MiB。GUI 再次校验 PNG 头、字节数与尺寸后才解码显示。超限、崩溃、超时、取消或不支持的 PDF 显示提示并保留系统阅读器打开路径。
- 该方案以 renderer 的全局 heap 分配预算满足 D30 的解码资源限制，绕开 macOS 不支持的 `RLIMIT_AS`。它不是 OS 级 RSS 上限；进程栈和系统分配器的管理开销不计入 heap 配额。
- PDF helper 随 macOS `.app`、Windows 安装器和 Linux tarball 一起分发，并签署 macOS helper；安装包同时附带 Hayro、PDFium 替代字体和嵌入色彩配置文件的许可说明。格式兼容、真实恶意样本及三平台打包 / 运行仍按开发计划验收。

## D51 音频预览范围与资源限制

- 压缩包详情栏支持 MP3、FLAC、OGG / Vorbis、WAV 和 M4A / AAC 音频；不自动播放。单条目压缩数据最多读取 8 MiB，解码 PCM 最多 18,000,000 个交错样本（72 MB），最长 3 分钟；只接受最多双声道、48 kHz 的流。超限或解码 / 音频设备不可用时提示用户用系统播放器打开。
- 归档读取与解码都在后台执行。音频输出由独立 worker 持有，切换条目、点击停止或关闭窗口会停止当前播放。解码器只接收有界字节数据，UI 线程不执行文件 I/O、解码或音频设备操作。
- 使用 Rodio 0.21.1 做播放队列、Symphonia 0.6.1 做纯 Rust 解复用 / 解码、CPAL 0.16.0 连接三平台系统音频输出；许可分别为 MIT OR Apache-2.0、MPL-2.0、Apache-2.0。Symphonia 0.6.1 的 fuzzing 修复、受限的容器签名识别和 WAV fmt 采样率 / 声道预检用于降低恶意输入触发解码 panic 的风险；MPL 依赖允许按文件级要求分发，源文件可由 Cargo.lock 中的 crates.io 版本获取。三平台发行包需附带其许可证文本；Linux 构建沿用现有 ALSA 开发库要求。
- nightly ASan fuzz target 调用同一个解码器，输入限制为 8 MiB、单次最多输出 500,000 个样本，并用项目生成的静音 WAV seed 起跑；播放器最多保留 72 MB 解码 PCM 并支持条目切换时取消解码。
- 其他视频编码和容器继续交给系统程序打开；H.264 首帧预览的准入与资源边界见 D52。

## D52 MP4 / QuickTime H.264 首帧预览

- 详情栏只为 MP4 / M4V / MOV 中的 H.264 `avc1` track 渲染第一张关键帧；要求 progressive、8-bit、4:2:0。仅支持非 fragmented MP4；HEVC、VP9、AV1、隔行、其他位深 / 色度格式和其他容器继续交给系统播放器。
- 归档条目最多读取 16 MiB。MP4 容器与 H.264 decoder 运行在 `rz-video-renderer` 子进程；共享分配器最多同时持有 256 MiB heap、累计最多 512 MiB，MP4 reader 最多执行 16,384 次读取 / seek，helper 与 GUI 父进程各有 10 秒时限。编码尺寸在解码前限制为最多 8,192×8,192 和 16,777,216 像素，参考帧 / 重排帧不超过 16；关键帧最多 8 MiB，PNG 最多 8 MiB / 16,777,216 像素。超限、损坏或不支持时保留系统播放器打开路径。
- 调用第三方 `mp4` parser 前先对完整 box tree 做范围检查：每个 box 必须落在父 box 内，深度最多 16、box 数最多 16,384；不接受 64-bit extended-size box。选中关键帧前校验 `stsc` 分块数和 `stts` 时间表覆盖，避免依赖库对损坏字段执行未检查的除法或 `unwrap()`。这会把合法的 extended-size MP4 排除在内置预览外，其他文件仍可用系统播放器打开。
- 使用 `mp4` 0.14.0（MIT）解析 ISO-MP4 / QuickTime 容器，使用 `rust_h264` 0.5.0（MIT OR Apache-2.0）解码；二者均为 Rust reader / decoder，不依赖系统编解码器或外部进程。发行包需携带两个依赖的完整许可证文本。
- `rust_h264` 0.5.0 以 upstream commit `c18b35e` 的 vendor 副本构建。局部补丁在 intra 4×4 luma block 写入前校验 frame plane 范围，损坏 slice 越界时返回 `InvalidSyntax`；原始 crash fixture 加入 renderer 单测和持续 Fuzz corpus。上游许可保持 MIT OR Apache-2.0，补丁说明见 `vendor/rust_h264/PATCHES.md`。
- nightly ASan `video_decoder` target 调用同一 renderer 核心，输入限制 16 MiB，CI seed 是合成的两帧 H.264 MP4 / MOV。fixture 生成来源见 `apps/video-renderer/tests/fixtures/README.md`。
- 这项功能只生成首帧缩略预览，不提供视频播放、音频轨播放或时间轴；超出受支持范围的文件仍由系统播放器处理。

## D53 ZIP 增量编辑仍采用暂存与原子替换

- 合格 ZIP 编辑先在归档同目录创建暂存文件。目标文件系统支持时，使用 copy-on-write reflink 克隆原归档；未改动条目继续指向原 local records，只为重命名 / 替换条目写新 local records，并重建 central directory。旧 central directory/footer 只会在暂存副本中调整。
- 完成后仍验证暂存归档，再原子替换原路径。reflink 或 append 处理不适用时回退既有 raw-copy 路径；不支持 ZIP 快路径的操作仍完整解压重打包。D11 的旧包保全边界不变。
- 这项优化减少支持 COW 的文件系统上对未改动数据的复制，不是原文件原地改写。若将来要直接改写原文件，必须先设计并验证恢复日志，再另行新增决策。
- `reflink-copy` 0.1.30 使用 MIT OR Apache-2.0；它只提供 Linux、macOS 和 Windows 的文件级克隆封装。MIT 文本随三平台安装包附带；任何平台或文件系统克隆失败都会回到 RustZIP 的 staged raw-copy 路径。
