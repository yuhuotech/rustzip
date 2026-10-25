import assert from "node:assert/strict";
import { test } from "node:test";
import { hashInstallId, telemetryReportSchema } from "../src/lib/telemetry";

const validReport = {
  event: "activate",
  install_id: "00000000-0000-4000-8000-000000000001",
  app_version: "0.1.0",
  os: "macos",
  os_version: "15.1",
  arch: "aarch64",
  language: "zh-CN",
};

test("accepts the documented activation and heartbeat payload", () => {
  assert.equal(telemetryReportSchema.safeParse(validReport).success, true);
  assert.equal(
    telemetryReportSchema.safeParse({ ...validReport, event: "heartbeat" }).success,
    true,
  );
});

test("rejects additional fields so the service cannot silently collect more data", () => {
  const result = telemetryReportSchema.safeParse({
    ...validReport,
    file_path: "/private/archive.zip",
  });
  assert.equal(result.success, false);
});

test("rejects unknown event names, non-v4 IDs, and malformed versions", () => {
  assert.equal(
    telemetryReportSchema.safeParse({ ...validReport, event: "file_opened" }).success,
    false,
  );
  assert.equal(
    telemetryReportSchema.safeParse({ ...validReport, install_id: "00000000-0000-1000-8000-000000000001" }).success,
    false,
  );
  assert.equal(
    telemetryReportSchema.safeParse({ ...validReport, app_version: "latest" }).success,
    false,
  );
});

test("HMAC protects the install UUID and is stable for the same key", () => {
  const secret = "a-stable-secret-for-local-tests-0123456789";
  const first = hashInstallId(validReport.install_id, secret);
  assert.equal(first, hashInstallId(validReport.install_id, secret));
  assert.notEqual(first, validReport.install_id);
  assert.notEqual(first, hashInstallId(validReport.install_id, "a-different-stable-secret"));
  assert.match(first, /^[0-9a-f]{64}$/);
});
