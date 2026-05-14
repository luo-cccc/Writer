export const DEFAULT_SITE_URL = "http://localhost:3000";

export function getSiteUrl(): string {
  return process.env.NEXT_PUBLIC_SITE_URL || process.env.SITE_URL || DEFAULT_SITE_URL;
}

export function getSiteLabel(): string {
  const configured = process.env.NEXT_PUBLIC_SITE_URL || process.env.SITE_URL;
  if (!configured) return "local draft";
  try {
    return new URL(configured).host;
  } catch {
    return configured;
  }
}
