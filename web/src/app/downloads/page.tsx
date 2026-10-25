import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "下载",
  description: "查看 RustZIP 的下载状态和从源码构建的方式。",
};

const platforms = [
  { name: "macOS", detail: "Apple Silicon 和 Intel", symbol: "⌘" },
  { name: "Windows", detail: "64 位桌面系统", symbol: "⊞" },
  { name: "Linux", detail: "64 位发行版", symbol: "◈" },
];

export default function DownloadsPage() {
  const buildCommands = [
    "git clone https://github.com/yuhuotech/rustzip.git",
    "cd rustzip",
    "cargo run --release -p rustzip-gui",
  ];

  return (
    <div className="inner-page">
      <section className="inner-hero container">
        <p className="section-kicker">下载 RustZIP</p>
        <h1>准备好之后，<br />就在这里见。</h1>
        <p className="inner-lede">RustZIP 仍处于早期开发阶段，正式安装包尚未发布。你可以先查看源码、跟进版本进度，或在本机自行构建。</p>
        <div className="hero-actions">
          <a className="button button-dark" href="https://github.com/yuhuotech/rustzip/releases" target="_blank" rel="noreferrer">查看 GitHub Releases <span aria-hidden="true">↗</span></a>
          <a className="button button-outline" href="https://github.com/yuhuotech/rustzip" target="_blank" rel="noreferrer">查看源代码</a>
        </div>
      </section>

      <section className="container download-status-section">
        <div className="release-status">
          <span className="status-icon">⌁</span>
          <div><small>当前状态</small><b>早期开发 · 暂无正式安装包</b></div>
          <span className="status-pill">持续开发中</span>
        </div>
        <h2>正在为这些平台打磨体验</h2>
        <div className="platform-grid download-platforms">
          {platforms.map((platform) => (
            <article className="platform-card" key={platform.name}>
              <span className="platform-mark">{platform.symbol}</span><b>{platform.name}</b><small>{platform.detail}</small>
            </article>
          ))}
        </div>
      </section>

      <section className="container build-panel">
        <div>
          <p className="section-kicker">开发者预览</p><h2>从源码运行</h2>
          <p>需要 Rust 1.93 或更新版本。仓库中的 `rust-toolchain.toml` 会选择项目工具链。</p>
        </div>
        <pre><code>{buildCommands.join("\n")}</code></pre>
      </section>
    </div>
  );
}
