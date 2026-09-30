// Formato de números, duraciones, tamaños y fechas con la configuración regional del sistema.

const locale = typeof navigator === "undefined" ? "es" : navigator.language || "es";
const NBSP = " ";
const integer = new Intl.NumberFormat(locale, { maximumFractionDigits: 0 });

export function formatInt(value: number) {
  return integer.format(Math.round(value));
}

export function formatDecimal(value: number, digits: number) {
  return new Intl.NumberFormat(locale, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
}

/** 23 868 → "23,9 k"; por debajo de mil, el número tal cual. */
export function formatThousands(value: number) {
  if (value < 1000) return formatInt(value);
  const digits = value >= 100_000 ? 0 : 1;
  return `${formatDecimal(value / 1000, digits)}${NBSP}k`;
}

export function formatPercent(value: number) {
  return `${formatInt(value)}${NBSP}%`;
}

/** Duración de la partida en minutos y segundos, truncada como en el cliente del juego. */
export function formatDuration(ms: number | null) {
  if (ms == null) return "—";
  return formatClock(Math.floor(ms / 1000));
}

export function formatClock(totalSeconds: number) {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function formatBytes(bytes: number) {
  if (bytes <= 0) return `0${NBSP}B`;
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  const digits = exponent === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  const number = new Intl.NumberFormat(locale, { maximumFractionDigits: digits }).format(value);
  return `${number}${NBSP}${units[exponent]}`;
}

export function formatDate(timestamp: number | null) {
  if (timestamp == null) return "—";
  return new Intl.DateTimeFormat(locale, {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

export function formatDateLong(timestamp: number | null) {
  if (timestamp == null) return "—";
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(timestamp),
  );
}

export function kdaRatio(kills: number, deaths: number, assists: number, digits = 2) {
  return deaths === 0 ? "Perfecto" : formatDecimal((kills + assists) / deaths, digits);
}
