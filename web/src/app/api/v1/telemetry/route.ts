import { persistTelemetry, usesLocalSqlite } from "@/lib/db";
import { hashInstallId, telemetryReportSchema } from "@/lib/telemetry";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";
export const maxDuration = 10;

const MAX_BODY_BYTES = 2_048;

function json(body: unknown, status = 200): Response {
  return Response.json(body, {
    status,
    headers: { "Cache-Control": "no-store" },
  });
}

async function readJson(request: Request): Promise<unknown | Response> {
  const contentLength = Number(request.headers.get("content-length"));
  if (Number.isFinite(contentLength) && contentLength > MAX_BODY_BYTES) {
    return json({ error: "payload_too_large" }, 413);
  }

  if (!request.body) {
    return json({ error: "invalid_json" }, 400);
  }

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BODY_BYTES) {
        await reader.cancel();
        return json({ error: "payload_too_large" }, 413);
      }
      chunks.push(value);
    }

    const body = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) {
      body.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(body));
  } catch {
    return json({ error: "invalid_json" }, 400);
  } finally {
    reader.releaseLock();
  }
}

export async function POST(request: Request) {
  if (!request.headers.get("content-type")?.toLowerCase().startsWith("application/json")) {
    return json({ error: "content_type_must_be_application_json" }, 415);
  }

  const body = await readJson(request);
  if (body instanceof Response) return body;

  const parsed = telemetryReportSchema.safeParse(body);
  if (!parsed.success) {
    return json({ error: "invalid_report" }, 400);
  }

  const secret =
    process.env.TELEMETRY_HASH_SECRET ||
    (process.env.NODE_ENV === "development" && usesLocalSqlite()
      ? "rustzip-local-only-telemetry-hmac-key"
      : undefined);
  if (!secret || secret.length < 32) {
    return json({ error: "service_unavailable" }, 503);
  }

  try {
    const report = parsed.data;
    const installHash = hashInstallId(report.install_id, secret);
    await persistTelemetry(report, installHash);

    return json({ accepted: true });
  } catch {
    return json({ error: "temporarily_unavailable" }, 503);
  }
}
