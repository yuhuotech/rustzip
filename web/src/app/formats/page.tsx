import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "格式支持",
  description: "查看 RustZIP 的读写、只读和仅识别格式范围。",
};

const groups = [
  {
    name: "读写",
    summary: "创建、浏览与解压",
    tone: "format-tone-accent",
    formats: ["ZIP", "7z", "TAR", "TAR.GZ", "TAR.BZ2", "TAR.XZ", "TAR.ZST", "TAR.LZ4", "GZ", "BZ2", "XZ", "ZST", "LZ4"],
    note: "ZIP 支持 AES-256。7z 支持 AES-256 和文件名加密。",
  },
  {
    name: "只读",
    summary: "浏览与解压",
    tone: "format-tone-blue",
    formats: ["RAR / RAR5", "ISO", "CAB", "WIM", "DMG", "CPIO", "AR", "RPM", "DEB", "CHM", "NSIS"],
    note: "RAR 只读，不创建 RAR 压缩包。长尾格式按后端能力提供只读支持。",
  },
  {
    name: "仅识别",
    summary: "识别并说明限制",
    tone: "format-tone-neutral",
    formats: ["其他可识别格式"],
    note: "格式无法处理时会提示原因，不会静默跳过或误报成功。",
  },
];

export default function FormatsPage() {
  return (
    <div className="inner-page">
      <section className="inner-hero container">
        <p className="section-kicker">清楚说明能力边界</p>
        <h1>格式支持，<br />不玩猜谜。</h1>
        <p className="inner-lede">RustZIP 按读写、只读和仅识别分档。格式判断看文件内容，不只看扩展名。</p>
      </section>
      <section className="container format-detail-list">
        {groups.map((group) => (
          <article className="format-detail-card" key={group.name}>
            <div className="format-detail-heading">
              <span className={`format-detail-symbol ${group.tone}`}>{group.name === "读写" ? "↔" : group.name === "只读" ? "↓" : "⌕"}</span>
              <div><h2>{group.name}</h2><p>{group.summary}</p></div>
            </div>
            <div className="format-chip-list">{group.formats.map((format) => <span className="format-chip" key={format}>{format}</span>)}</div>
            <p className="format-note"><span aria-hidden="true">ⓘ</span>{group.note}</p>
          </article>
        ))}
      </section>
      <section className="container format-safety-note">
        <span className="safety-shield">◇</span>
        <div><b>不确定的压缩包，也会先做安全检查</b><p>压缩包内容都视为不可信输入。解压时会检查路径，防止文件写到目标目录之外。</p></div>
      </section>
    </div>
  );
}
