import {
  index,
  integer,
  primaryKey,
  sqliteTable,
  text,
} from "drizzle-orm/sqlite-core";

export const telemetryInstallsSqlite = sqliteTable(
  "telemetry_installs",
  {
    installHash: text("install_hash").primaryKey(),
    firstSeenAt: integer("first_seen_at", { mode: "timestamp_ms" }).notNull(),
    lastSeenAt: integer("last_seen_at", { mode: "timestamp_ms" }).notNull(),
    appVersion: text("app_version").notNull(),
    os: text("os").notNull(),
    osVersion: text("os_version").notNull(),
    arch: text("arch").notNull(),
    language: text("language").notNull(),
  },
  (table) => [index("telemetry_installs_first_seen_idx").on(table.firstSeenAt)],
);

export const telemetryDailyActivitySqlite = sqliteTable(
  "telemetry_daily_activity",
  {
    installHash: text("install_hash")
      .notNull()
      .references(() => telemetryInstallsSqlite.installHash, { onDelete: "cascade" }),
    activityDate: text("activity_date").notNull(),
    event: text("event").notNull(),
  },
  (table) => [
    primaryKey({ columns: [table.installHash, table.activityDate] }),
    index("telemetry_daily_activity_date_idx").on(table.activityDate),
  ],
);
