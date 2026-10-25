import Link from "next/link";

export function SiteFooter() {
  return (
    <footer className="site-footer">
      <div className="container footer-inner">
        <Link className="brand footer-brand" href="/">
          <span className="brand-mark" aria-hidden="true"><span /></span>
          <span>RustZIP</span>
        </Link>
        <p>用 Rust 打造，开源、跨平台的压缩管理工具。</p>
        <nav aria-label="页脚导航">
          <Link href="/formats">格式支持</Link>
          <Link href="/downloads">下载</Link>
          <Link href="/privacy">隐私说明</Link>
          <a href="https://github.com/yuhuotech/rustzip" target="_blank" rel="noreferrer">GitHub ↗</a>
        </nav>
        <small>© {new Date().getFullYear()} RustZIP · MIT OR Apache-2.0</small>
      </div>
    </footer>
  );
}
