import { createClient } from "@libsql/client";
import { neon } from "@neondatabase/serverless";
import { sql } from "drizzle-orm";
import { drizzle as drizzleLibsql } from "drizzle-orm/libsql";
import { drizzle as drizzleNeon } from "drizzle-orm/neon-http";
import type { TelemetryReport } from "../telemetry";
import * as postgresSchema from "./schema";
import * as sqliteSchema from "./schema.sqlite";

function createSqliteDatabase() {
  const url = process.env.LOCAL_DATABASE_URL ?? "file:./rustzip.local.db";
  return drizzleLibsql({ client: createClient({ url }), schema: sqliteSchema });
}

function createPostgresDatabase() {
  const url = process.env.DATABASE_URL;
  if (!url) {
    throw new Error("DATABASE_URL is not configured");
  }
  return drizzleNeon({ client: neon(url), schema: postgresSchema });
}

let sqliteDatabase: ReturnType<typeof createSqliteDatabase> | undefined;
let postgresDatabase: ReturnType<typeof createPostgresDatabase> | undefined;

export function usesLocalSqlite(): boolean {
  const configuredProvider = process.env.RZ_DATABASE_PROVIDER;
  if (configuredProvider === "sqlite") return true;
  if (configuredProvider === "postgres") return false;
  return process.env.NODE_ENV !== "production";
}

export async function checkDatabase(): Promise<void> {
  if (usesLocalSqlite()) {
    sqliteDatabase ??= createSqliteDatabase();
    await sqliteDatabase.run(sql`select 1`);
    return;
  }

  postgresDatabase ??= createPostgresDatabase();
  await postgresDatabase.execute(sql`select 1`);
}

export async function persistTelemetry(
  report: TelemetryReport,
  installHash: string,
): Promise<void> {
  const now = new Date();
  const activityDate = now.toISOString().slice(0, 10);

  if (usesLocalSqlite()) {
    sqliteDatabase ??= createSqliteDatabase();
    await sqliteDatabase
      .insert(sqliteSchema.telemetryInstallsSqlite)
      .values({
        installHash,
        firstSeenAt: now,
        lastSeenAt: now,
        appVersion: report.app_version,
        os: report.os,
        osVersion: report.os_version,
        arch: report.arch,
        language: report.language,
      })
      .onConflictDoUpdate({
        target: sqliteSchema.telemetryInstallsSqlite.installHash,
        set: {
          lastSeenAt: now,
          appVersion: report.app_version,
          os: report.os,
          osVersion: report.os_version,
          arch: report.arch,
          language: report.language,
        },
      });

    await sqliteDatabase
      .insert(sqliteSchema.telemetryDailyActivitySqlite)
      .values({ installHash, activityDate, event: report.event })
      .onConflictDoNothing();
    return;
  }

  postgresDatabase ??= createPostgresDatabase();
  await postgresDatabase
    .insert(postgresSchema.telemetryInstalls)
    .values({
      installHash,
      appVersion: report.app_version,
      os: report.os,
      osVersion: report.os_version,
      arch: report.arch,
      language: report.language,
    })
    .onConflictDoUpdate({
      target: postgresSchema.telemetryInstalls.installHash,
      set: {
        lastSeenAt: now,
        appVersion: report.app_version,
        os: report.os,
        osVersion: report.os_version,
        arch: report.arch,
        language: report.language,
      },
    });

  await postgresDatabase
    .insert(postgresSchema.telemetryDailyActivity)
    .values({ installHash, activityDate, event: report.event })
    .onConflictDoNothing();
}
