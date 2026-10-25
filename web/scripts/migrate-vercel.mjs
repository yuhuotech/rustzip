import { spawnSync } from "node:child_process";

if (process.env.VERCEL !== "1") {
  process.stdout.write("Skipping Neon migration outside a Vercel build.\n");
  process.exit(0);
}

if (!process.env.DATABASE_URL || !process.env.DATABASE_URL_UNPOOLED) {
  process.stderr.write(
    "Vercel builds require DATABASE_URL and DATABASE_URL_UNPOOLED from the Neon integration.\n",
  );
  process.exit(1);
}

if (!process.env.TELEMETRY_HASH_SECRET || process.env.TELEMETRY_HASH_SECRET.length < 32) {
  process.stderr.write(
    "Set a random TELEMETRY_HASH_SECRET of at least 32 characters in this Vercel environment.\n",
  );
  process.exit(1);
}

process.stdout.write("Applying Neon Postgres migrations for this Vercel environment.\n");
const result = spawnSync(
  process.execPath,
  ["node_modules/drizzle-kit/bin.cjs", "migrate", "--config=drizzle.postgres.config.ts"],
  { stdio: "inherit" },
);

if (result.error) {
  process.stderr.write(`${result.error.message}\n`);
  process.exit(1);
}

process.exit(result.status ?? 1);
