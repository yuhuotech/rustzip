import {
  date,
  index,
  pgTable,
  primaryKey,
  timestamp,
  varchar,
} from "drizzle-orm/pg-core";

export const telemetryInstalls = pgTable(
  "telemetry_installs",
  {
    installHash: varchar("install_hash", { length: 64 }).primaryKey(),
    firstSeenAt: timestamp("first_seen_at", { withTimezone: true })
      .notNull()
      .defaultNow(),
    lastSeenAt: timestamp("last_seen_at", { withTimezone: true })
      .notNull()
      .defaultNow(),
    appVersion: varchar("app_version", { length: 64 }).notNull(),
    os: varchar("os", { length: 16 }).notNull(),
    osVersion: varchar("os_version", { length: 128 }).notNull(),
    arch: varchar("arch", { length: 32 }).notNull(),
    language: varchar("language", { length: 64 }).notNull(),
  },
  (table) => [index("telemetry_installs_first_seen_idx").on(table.firstSeenAt)],
);

export const telemetryDailyActivity = pgTable(
  "telemetry_daily_activity",
  {
    installHash: varchar("install_hash", { length: 64 })
      .notNull()
      .references(() => telemetryInstalls.installHash, { onDelete: "cascade" }),
    activityDate: date("activity_date").notNull(),
    event: varchar("event", { length: 16 }).notNull(),
  },
  (table) => [
    primaryKey({ columns: [table.installHash, table.activityDate] }),
    index("telemetry_daily_activity_date_idx").on(table.activityDate),
  ],
);
