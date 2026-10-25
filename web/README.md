# RustZIP Web

`web/` 是独立的 Next.js 应用，包含 RustZIP 官网和桌面应用匿名安装统计接收 API。本机 Next.js 进程监听 `0.0.0.0:8035`，由 Nginx 反向代理到无端口域名。

## 技术栈

- Next.js 16 + React + TypeScript：官网页面与 Node.js Route Handler API。
- 本地开发：SQLite 文件数据库，Drizzle 使用 `@libsql/client`；无需安装或连接 PostgreSQL。
- Vercel Preview / Production：Neon Postgres，通过 Vercel Marketplace 集成连接；函数使用 Neon serverless HTTP driver。
- Drizzle ORM / Drizzle Kit：分别维护 SQLite 与 Postgres schema 和迁移。
- Zod：严格验证客户端已有的 7 字段统计协议。

依赖许可证已核对：Next.js / React、Neon driver、libSQL client/runtime、Zod、tsx、`pg`、类型声明为 MIT；Drizzle ORM 和 TypeScript 为 Apache-2.0；Drizzle Kit 为 MIT；dotenv 为 BSD-2-Clause。均为宽松许可证，符合仓库依赖限制。

## 页面与接口

| 路径 | 作用 |
|---|---|
| `/` | 官网首页 |
| `/downloads` | 当前发布状态和源码构建方式 |
| `/formats` | 读写、只读、仅识别格式说明 |
| `/privacy` | 官网和匿名统计隐私说明 |
| `GET /api/health` | 检查 API 与当前环境数据库连接 |
| `POST /api/v1/telemetry` | 接收桌面应用的激活和每日心跳 |

桌面端上报 JSON 与 `rz_app::stats::Report` 一致：

```json
{
  "event": "activate",
  "install_id": "00000000-0000-4000-8000-000000000001",
  "app_version": "0.1.0",
  "os": "macos",
  "os_version": "15.1",
  "arch": "aarch64",
  "language": "zh-CN"
}
```

API 只接受 `event=activate|heartbeat` 和这 7 个字段，额外字段会拒绝；请求体最大 2 KiB。应用 UUID 在数据库写入前通过 `TELEMETRY_HASH_SECRET` 做 HMAC-SHA-256。数据库保存摘要、首次 / 最近上报时间、最近环境信息，以及以“摘要 + UTC 日期”为主键的每日活跃记录。重复提交幂等，不会重复计算安装数或 DAU。接口不读取或写入请求 IP，也不输出上报内容日志。

更新清单仍由 GitHub Releases 提供并由 Rust 客户端验签。本服务不代理、不重新签发更新包。

## 首次部署

1. 在 Vercel 导入仓库，新建独立项目，将 **Root Directory** 设为 `web`；使用默认 Next.js Install / Build 设置。
2. 在 Vercel Marketplace 安装 Neon Postgres 并连接这个 Vercel 项目，勾选 Development、Preview、Production。建议为 Preview 部署启用独立 Neon 分支。
3. Vercel Neon 集成自动注入 `DATABASE_URL`（连接池）和 `DATABASE_URL_UNPOOLED`（直连）。若手动连接 Neon，也使用这两个变量名，并让它们指向同一个 Neon database/branch；数据库可命名为 `rustzip`，应用不会硬编码数据库名。API 用 `DATABASE_URL`，迁移用 `DATABASE_URL_UNPOOLED`。为 Vercel Preview、Production 配置随机 `TELEMETRY_HASH_SECRET`（至少 32 个字符，Production 的值跨部署保持不变）。Development 配置 `SITE_URL=https://rustzip.dev.yuhuotech.cn`；Production 配置 `SITE_URL=https://rustzip.vercel.app`。
4. 连接 Neon 后直接部署。Vercel 的 `npm run build` 会先检查两个数据库 URL 和哈希密钥，再自动执行 Postgres migrations，最后构建 Next.js。迁移按 Drizzle migration ledger 幂等执行；缺少必需变量或迁移失败会让部署失败，避免发布没有数据库表的 API。
5. 部署 Preview 并验证；Preview 使用 Neon 集成创建的隔离分支，部署时会在该分支应用同一组 migrations。
6. 首次部署后，将 `https://rustzip.vercel.app/api/v1/telemetry` 设置为 GitHub 仓库 Variable `RZ_STATS_URL`。以后将自定义域名切换到 `rustzip.com` 时，只需更新 `SITE_URL` 和 `RZ_STATS_URL`；接口路径、Vercel 项目和 Neon 数据库保持不变。重新构建/发布桌面安装包后，客户端才会使用新的统计地址。

`DATABASE_URL` 和 `TELEMETRY_HASH_SECRET` 都是服务端变量，不要使用 `NEXT_PUBLIC_` 前缀或提交实际 `.env` 文件。根目录 `.gitignore` 已忽略 `web/.env*` 本地密钥文件，并保留 `.env.example`。

## 本地开发、测试与验证

本地 `npm run dev` 默认使用 `web/rustzip.local.db` 这个 SQLite 文件。启动时 `predev` 会自动应用 SQLite 迁移；不用安装 PostgreSQL、不用拉取 Vercel 数据库变量，也不会把本地上报写入线上。这个本地数据库文件已加入 `.gitignore`。Vercel Preview / Production 在 `NODE_ENV=production` 下使用 Neon；Vercel 集成会为 Preview 提供独立分支，避免测试数据进入 Production。

Next.js 监听 `0.0.0.0:8035`；Nginx 把 `rustzip.dev.yuhuotech.cn` 的 HTTPS 请求反向代理到 `http://127.0.0.1:8035`。浏览器访问 `https://rustzip.dev.yuhuotech.cn` 时不需要写端口。

Nginx 的 `location /` 可按现有 HTTPS server 配置转发到这个 upstream：

```nginx
location / {
    proxy_pass http://127.0.0.1:8035;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

```bash
cd web
npm ci
npm run dev
```

默认 SQLite 文件名是 `rustzip.local.db`，可用 `LOCAL_DATABASE_URL=file:./其他文件名.db` 修改。开发模式下若未设置 `TELEMETRY_HASH_SECRET`，服务使用仅限本地开发的固定测试密钥；Vercel Preview / Production 必须配置各自的强随机密钥。Development 域名变量可通过 `.env.local` 设置 `SITE_URL=https://rustzip.dev.yuhuotech.cn`。

本地 SQLite 数据和 Vercel Neon 数据彼此独立，不会同步。若需要在本地调试 Postgres，可显式设置 `RZ_DATABASE_PROVIDER=postgres` 并配置一个非生产 `DATABASE_URL`；日常官网与接口开发保持 SQLite 即可。

`npm run build` 会依次执行 TypeScript 检查、接口 schema 单元测试和 Next.js production build；这也是 Vercel 部署使用的 build 命令。

验证页面与数据库（经 Nginx 域名）：

```bash
curl -i https://rustzip.dev.yuhuotech.cn/api/health
```

成功应返回 HTTP 200 和 `{"status":"ok","database":"ok"}`。没有有效数据库配置时应返回 503。绕过 Nginx 检查本机 Next.js 进程可请求 `http://127.0.0.1:8035/api/health`。

验证统计接收、重复事件去重和字段白名单：

```bash
curl -i https://rustzip.dev.yuhuotech.cn/api/v1/telemetry \
  -H 'content-type: application/json' \
  --data '{"event":"activate","install_id":"00000000-0000-4000-8000-000000000001","app_version":"0.1.0","os":"macos","os_version":"15.1","arch":"aarch64","language":"zh-CN"}'
```

预期响应 HTTP 200 `{"accepted":true}`。使用相同请求重复发送后，数据库仍只有一个 `telemetry_installs` 安装项和当天一条 `telemetry_daily_activity`。添加额外字段或改成未知 event 时应返回 400；非 JSON 返回 415；请求体超过 2 KiB 返回 413。

本地可用 `npm run db:studio` 查看 SQLite 数据；Vercel Neon 可用 `npm run db:studio:postgres`（需先注入对应 Vercel 环境变量）或 Neon SQL Editor。查询语句在 SQLite 和 Postgres 中相同：

```sql
select count(*) as activated_installs from telemetry_installs;

select activity_date, count(*) as daily_active_installs
from telemetry_daily_activity
group by activity_date
order by activity_date desc;
```

## 数据库迁移

本地 SQLite schema 在 `src/lib/db/schema.sqlite.ts`，Neon Postgres schema 在 `src/lib/db/schema.ts`。修改任一 schema 后使用对应脚本生成并检查迁移：

```bash
npm run db:generate                 # SQLite
npm run db:generate:postgres        # Postgres
npm run db:migrate                  # 本地 SQLite
npm run db:migrate:postgres         # Neon Postgres
```

迁移分别保存在 `drizzle/sqlite/` 和 `drizzle/postgres/`。先验证 SQLite，再在 Neon Preview 分支检查 Postgres 迁移；确认后再通过 Vercel Production 环境明确应用生产迁移。

## 当前范围

- 官网没有第三方分析脚本、广告、账号或下载统计接口。
- 客户端现在只上报首次激活和每日心跳，不增加文件打开、压缩、解压等使用行为日志。
- GitHub Release / CDN 下载量继续通过发布渠道查看，不在 Web API 重复采集。

## 端口与域名

- 本地 Next.js `dev` 和自托管 `start` 命令都绑定 `0.0.0.0:8035`。
- 本地域名：`rustzip.dev.yuhuotech.cn`，由 Nginx 终止 HTTPS 并代理到 `127.0.0.1:8035`；开发环境 `SITE_URL` 设为 `https://rustzip.dev.yuhuotech.cn`。
- 当前生产域名：`rustzip.vercel.app`，由 Vercel 提供 HTTPS。完成备案后可把 `rustzip.com` 绑定到同一 Vercel 项目，并更新 Production 的 `SITE_URL` 与 GitHub Variable `RZ_STATS_URL`；API 和 Neon 数据库无需迁移。
