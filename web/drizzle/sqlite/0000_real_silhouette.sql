CREATE TABLE `telemetry_daily_activity` (
	`install_hash` text NOT NULL,
	`activity_date` text NOT NULL,
	`event` text NOT NULL,
	PRIMARY KEY(`install_hash`, `activity_date`),
	FOREIGN KEY (`install_hash`) REFERENCES `telemetry_installs`(`install_hash`) ON UPDATE no action ON DELETE cascade
);
--> statement-breakpoint
CREATE INDEX `telemetry_daily_activity_date_idx` ON `telemetry_daily_activity` (`activity_date`);--> statement-breakpoint
CREATE TABLE `telemetry_installs` (
	`install_hash` text PRIMARY KEY NOT NULL,
	`first_seen_at` integer NOT NULL,
	`last_seen_at` integer NOT NULL,
	`app_version` text NOT NULL,
	`os` text NOT NULL,
	`os_version` text NOT NULL,
	`arch` text NOT NULL,
	`language` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `telemetry_installs_first_seen_idx` ON `telemetry_installs` (`first_seen_at`);