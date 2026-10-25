import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "隐私说明",
  description: "RustZIP 官网与桌面应用匿名安装统计的隐私说明。",
};

export default function PrivacyPage() {
  return (
    <div className="inner-page privacy-page">
      <section className="inner-hero container">
        <p className="section-kicker">隐私说明</p>
        <h1>少收集，<br />说清楚。</h1>
        <p className="inner-lede">RustZIP 官网不使用广告或访问行为统计。桌面应用的匿名安装统计需要你先同意，也可以随时关闭。</p>
      </section>
      <article className="container privacy-content">
        <section>
          <h2>桌面应用会发送什么</h2>
          <p>只有你在首次启动提示中选择参与后，应用才会发送以下字段：</p>
          <ul>
            <li>随机生成的安装 UUID；</li>
            <li>事件类型：首次激活 `activate`，之后每天最多一次 `heartbeat`；</li>
            <li>RustZIP 版本、操作系统及版本、CPU 架构、界面语言。</li>
          </ul>
          <p>应用不发送用户名、文件名、文件路径、文件内容或压缩操作行为。设置里可以随时关闭统计。</p>
        </section>
        <section>
          <h2>服务端如何保存</h2>
          <p>接收接口在写入数据库前，会用服务端密钥对 UUID 做 HMAC-SHA-256。数据库保存此摘要、首次和最近上报时间、最近一次环境信息，以及按天去重的活跃记录。原始 UUID 不写入数据库；RustZIP 接口代码不读取或保存请求 IP。</p>
          <p>摘要记录用于去重激活和计算每日活跃安装数，保留在统计数据库中。上报失败时应用不会阻塞使用；关闭统计后不会再发送新记录。</p>
        </section>
        <section>
          <h2>官网访问</h2>
          <p>官网不嵌入第三方分析、广告像素、社交媒体追踪或用户账号系统。网站托管与数据库服务由 Vercel 和 Neon 提供；它们可能依据各自服务条款处理运行所需的技术日志。</p>
        </section>
        <section>
          <h2>更新检查</h2>
          <p>桌面应用可从 GitHub Releases 检查更新。更新文件会由应用内置公钥验签；更新检查可以在应用设置中关闭。官网不会替应用下载或执行更新。</p>
        </section>
        <p className="privacy-updated">项目代码与数据字段说明见 GitHub 仓库。此页面随实际收集内容一起维护。</p>
      </article>
    </div>
  );
}
