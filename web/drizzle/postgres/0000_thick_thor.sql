CREATE TABLE "telemetry_daily_activity" (
	"install_hash" varchar(64) NOT NULL,
	"activity_date" date NOT NULL,
	"event" varchar(16) NOT NULL,
	CONSTRAINT "telemetry_daily_activity_install_hash_activity_date_pk" PRIMARY KEY("install_hash","activity_date")
);
--> statement-breakpoint
CREATE TABLE "telemetry_installs" (
	"install_hash" varchar(64) PRIMARY KEY NOT NULL,
	"first_seen_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_seen_at" timestamp with time zone DEFAULT now() NOT NULL,
	"app_version" varchar(64) NOT NULL,
	"os" varchar(16) NOT NULL,
	"os_version" varchar(128) NOT NULL,
	"arch" varchar(32) NOT NULL,
	"language" varchar(64) NOT NULL
);
--> statement-breakpoint
ALTER TABLE "telemetry_daily_activity" ADD CONSTRAINT "telemetry_daily_activity_install_hash_telemetry_installs_install_hash_fk" FOREIGN KEY ("install_hash") REFERENCES "public"."telemetry_installs"("install_hash") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "telemetry_daily_activity_date_idx" ON "telemetry_daily_activity" USING btree ("activity_date");--> statement-breakpoint
CREATE INDEX "telemetry_installs_first_seen_idx" ON "telemetry_installs" USING btree ("first_seen_at");