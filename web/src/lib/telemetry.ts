import { createHmac } from "node:crypto";
import { z } from "zod";

const uuidV4 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export const telemetryReportSchema = z
  .object({
    event: z.enum(["activate", "heartbeat"]),
    install_id: z.string().regex(uuidV4),
    app_version: z
      .string()
      .max(64)
      .regex(/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/),
    os: z.enum(["macos", "windows", "linux"]),
    os_version: z.string().min(1).max(128).regex(/^[A-Za-z0-9 ._+()/-]+$/),
    arch: z.string().min(1).max(32).regex(/^[A-Za-z0-9._-]+$/),
    language: z.string().min(1).max(64).regex(/^[A-Za-z0-9_-]+$/),
  })
  .strict();

export type TelemetryReport = z.infer<typeof telemetryReportSchema>;

export function hashInstallId(installId: string, secret: string): string {
  return createHmac("sha256", secret).update(installId.toLowerCase()).digest("hex");
}
