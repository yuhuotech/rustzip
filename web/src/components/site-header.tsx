import Link from "next/link";

const links = [
  { href: "/#features", label: "功能" },
  { href: "/formats", label: "格式支持" },
  { href: "/downloads", label: "下载" },
  { href: "/privacy", label: "隐私" },
];

export function SiteHeader() {
  return (
    <header className="site-header">
      <div className="container header-inner">
        <Link className="brand" href="/" aria-label="RustZIP 首页">
          <span className="brand-mark" aria-hidden="true"><span /></span>
          <span>RustZIP</span>
        </Link>
        <nav className="main-nav" aria-label="主导航">
          {links.map((link) => <Link href={link.href} key={link.href}>{link.label}</Link>)}
        </nav>
        <a className="button button-small button-dark header-cta" href="https://github.com/yuhuotech/rustzip" target="_blank" rel="noreferrer">
          GitHub <span aria-hidden="true">↗</span>
        </a>
      </div>
    </header>
  );
}
