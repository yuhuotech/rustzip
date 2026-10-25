import Image from "next/image";
import Link from "next/link";

const features = [
  {
    number: "01",
    title: "常见格式，一处管理",
    description: "创建、浏览和解压 ZIP、7z、TAR 及常见压缩流，也能只读打开 RAR。",
    tag: "READ · WRITE · EXTRACT",
  },
  {
    number: "02",
    title: "流式处理，少等一会",
    description: "大文件按流水线处理，并行度和内存受控，让速度与资源占用都有边界。",
    tag: "STREAMING · PARALLEL",
  },
  {
    number: "03",
    title: "把文件安全放回原处",
    description: "解压前检查路径和目标，避免路径逃逸；失败时不留下半成品。",
    tag: "SAFE BY DEFAULT",
  },
  {
    number: "04",
    title: "图形界面与命令行共用核心",
    description: "日常操作用桌面应用，自动化任务用 rz 命令，两边行为保持一致。",
    tag: "DESKTOP · CLI",
  },
];

const platforms = [
  { name: "macOS", detail: "Apple Silicon · Intel", mark: "⌘" },
  { name: "Windows", detail: "x86_64", mark: "⊞" },
  { name: "Linux", detail: "x86_64 · 更多平台持续验证", mark: "◈" },
];

export default function HomePage() {
  return (
    <>
      <section className="hero-section">
        <div className="hero-glow" aria-hidden="true" />
        <div className="container hero-grid">
          <div className="hero-copy">
            <div className="eyebrow"><span className="eyebrow-dot" />开源 · 跨平台 · Rust 构建</div>
            <h1>压缩文件，<br /><span>简单一点。</span></h1>
            <p className="hero-lede">
              RustZIP 把压缩、解压、浏览、转换和修复放进一个轻快的工具里。界面清晰，文件安全，常用格式一站搞定。
            </p>
            <div className="hero-actions">
              <Link className="button button-accent" href="/downloads">获取 RustZIP <span aria-hidden="true">↗</span></Link>
              <a className="button button-outline" href="https://github.com/yuhuotech/rustzip" target="_blank" rel="noreferrer">浏览源代码</a>
            </div>
            <div className="hero-note">
              <span className="mini-platforms" aria-label="目标平台：macOS、Windows 和 Linux"><span>⌘</span><span>⊞</span><span>◈</span></span>
              面向 macOS、Windows 和 Linux 开发
            </div>
          </div>

          <div className="hero-art" aria-label="RustZIP 压缩包浏览界面预览">
            <div className="hero-orbit orbit-one" />
            <div className="hero-orbit orbit-two" />
            <div className="app-window">
              <div className="app-window-top">
                <div className="window-dots"><i /><i /><i /></div>
                <span className="app-window-title">RustZIP <span>·</span> 项目资料.zip</span>
                <span className="window-menu">•••</span>
              </div>
              <Image
                src="/browse-light.png"
                alt="RustZIP 浏览压缩包的桌面界面"
                width={1600}
                height={1000}
                priority
                className="app-screenshot"
                sizes="(max-width: 900px) 92vw, 640px"
              />
              <div className="preview-float">
                <span className="preview-check">✓</span>
                <span><b>安全检查已启用</b><small>路径规范化 · 同名保护</small></span>
              </div>
            </div>
            <div className="hero-caption"><span /> 界面预览 · macOS</div>
          </div>
        </div>
        <div className="container hero-trust">
          <span>ZIP</span><span>7z</span><span>TAR</span><span>RAR 只读</span>
          <i />
          <span>无需账号</span><span>无广告</span><span>隐私优先</span>
        </div>
      </section>

      <section className="section feature-section" id="features">
        <div className="container">
          <div className="section-heading split-heading">
            <div><p className="section-kicker">为每天的文件整理而做</p><h2>够快，也够顺手。</h2></div>
            <p className="section-intro">常用操作一眼就能找到。复杂格式和高级选项收在需要时再用的地方。</p>
          </div>
          <div className="feature-grid">
            {features.map((feature) => (
              <article className="feature-card" key={feature.number}>
                <div className="feature-card-top"><span>{feature.number}</span><i aria-hidden="true">↗</i></div>
                <h3>{feature.title}</h3><p>{feature.description}</p><small>{feature.tag}</small>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section className="section format-teaser">
        <div className="container format-teaser-grid">
          <div>
            <p className="section-kicker">格式支持清清楚楚</p>
            <h2>能做什么，<br />一目了然。</h2>
            <p className="section-intro">可读写、只读和仅识别分档说明。RAR 只提供浏览和解压，绝不创建 RAR 压缩包。</p>
            <Link className="text-link" href="/formats">查看完整格式列表 <span aria-hidden="true">→</span></Link>
          </div>
          <div className="format-stack">
            <div className="format-row format-row-primary">
              <span className="format-icon">↔</span><div><b>读写</b><small>创建、浏览、解压</small></div>
              <span className="format-list">ZIP · 7z · TAR · GZ · XZ · ZST</span><span className="format-arrow">↗</span>
            </div>
            <div className="format-row">
              <span className="format-icon muted-icon">↓</span><div><b>只读</b><small>浏览、解压</small></div>
              <span className="format-list">RAR · ISO · CAB · WIM</span><span className="format-arrow">↗</span>
            </div>
            <div className="format-row">
              <span className="format-icon muted-icon">⌕</span><div><b>仅识别</b><small>识别后说明限制</small></div>
              <span className="format-list">其他已知格式</span><span className="format-arrow">↗</span>
            </div>
            <div className="format-footnote"><span>ⓘ</span>不支持的格式会明确提示，不会假装已经解压。</div>
          </div>
        </div>
      </section>

      <section className="section platform-section">
        <div className="container">
          <div className="section-heading centered-heading">
            <p className="section-kicker">一个工具，各个平台</p><h2>你的电脑，都能用。</h2>
            <p className="section-intro">目标覆盖三大桌面平台；Windows 和 Linux 的真机体验仍在验证中。</p>
          </div>
          <div className="platform-grid">
            {platforms.map((platform) => (
              <div className="platform-card" key={platform.name}>
                <span className="platform-mark">{platform.mark}</span><b>{platform.name}</b><small>{platform.detail}</small>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="closing-section">
        <div className="container closing-card">
          <div className="closing-spark" aria-hidden="true">✳</div>
          <div>
            <p className="section-kicker">开源项目，持续打磨</p>
            <h2>和 RustZIP 一起，<br className="mobile-break" />把文件整理好。</h2>
            <p>查看项目进度、参与讨论，或从源码开始体验。</p>
          </div>
          <div className="closing-actions">
            <Link className="button button-dark" href="/downloads">查看下载方式</Link>
            <a className="text-link light-link" href="https://github.com/yuhuotech/rustzip" target="_blank" rel="noreferrer">前往 GitHub <span aria-hidden="true">↗</span></a>
          </div>
        </div>
      </section>
    </>
  );
}
