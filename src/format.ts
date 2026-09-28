const numberFormat = new Intl.NumberFormat("en-US");

export function formatNumber(value: number): string {
  return numberFormat.format(value);
}

export function formatCompact(value: number): string {
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

export function formatDuration(ms: number | null): string {
  if (!ms) return "–";
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  return `${minutes}:${String(total % 60).padStart(2, "0")}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

export function formatRelative(ms: number | null, now = Date.now()): string {
  if (ms === null) return "";
  const diff = Math.max(0, now - ms);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (diff < minute) return "now";
  if (diff < hour) return `${Math.floor(diff / minute)}m ago`;
  if (diff < day) return `${Math.floor(diff / hour)}h ago`;
  if (diff < 7 * day) return `${Math.floor(diff / day)}d ago`;
  const date = new Date(ms);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}

export function formatDate(ms: number | null): string {
  if (ms === null) return "";
  return new Date(ms).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

const POSITIONS: Record<string, string> = {
  TOP: "Top",
  JUNGLE: "Jungle",
  MIDDLE: "Mid",
  MID: "Mid",
  BOTTOM: "Bot",
  BOT: "Bot",
  ADC: "Bot",
  UTILITY: "Support",
  SUPPORT: "Support",
};

export function positionLabel(position: string): string {
  const key = position.trim().toUpperCase();
  if (!key || key === "NONE" || key === "INVALID") return "–";
  return POSITIONS[key] ?? position;
}

export function stageLabel(stage: string | undefined): string {
  switch (stage) {
    case "resolve":
      return "Finding client release";
    case "manifest":
      return "Downloading manifest";
    case "plan":
      return "Planning files";
    case "download":
      return "Downloading files";
    case "repair":
      return "Repairing files";
    case "complete":
      return "Complete";
    case "cancelled":
      return "Cancelled";
    case "failed":
      return "Failed";
    default:
      return stage ?? "Working";
  }
}

const LOCALE_PATTERN = /^[a-z]{2}_[A-Z]{2}$/;

export function isValidLocale(value: string): boolean {
  return LOCALE_PATTERN.test(value);
}

/** Separator-aware join that follows whatever style the base path already uses. */
export function joinPath(base: string, name: string): string {
  const separator = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  const trimmed = base.replace(/[\\/]+$/, "");
  return `${trimmed}${separator}${name}`;
}

export function stripExtension(fileName: string): string {
  return fileName.replace(/\.rofl$/i, "");
}

export function teamLabel(team: string | null): string {
  if (!team) return "";
  const lower = team.toLowerCase();
  if (lower === "blue" || lower === "100") return "Blue";
  if (lower === "red" || lower === "200") return "Red";
  return team;
}
