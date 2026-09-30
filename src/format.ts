// Formato de números, duraciones, tamaños y fechas. La interfaz está en español: se usa la variante
// del sistema si es español (es-MX, es-ES…) y, si no, "es". Cada formateador se crea una sola vez.

const NBSP = " ";
const locale = interfaceLocale();
const integer = new Intl.NumberFormat(locale, { maximumFractionDigits: 0 });
// Hasta 0, 1 o 2 decimales, sin ceros finales (tamaños).
const upTo = [0, 1, 2].map((digits) => new Intl.NumberFormat(locale, { maximumFractionDigits: digits }));
// Exactamente N decimales (promedios, miles), creados al primer uso de cada N.
const fixed = new Map<number, Intl.NumberFormat>();
const dateShort = new Intl.DateTimeFormat(locale, {
  day: "numeric",
  month: "short",
  hour: "2-digit",
  minute: "2-digit",
});
const dateLong = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB"];

function interfaceLocale() {
  const system = typeof navigator === "undefined" ? "" : navigator.language ?? "";
  if (!/^es(?:-|$)/i.test(system)) return "es";
  try {
    new Intl.NumberFormat(system);
    return system;
  } catch {
    return "es";
  }
}

function roundTo(value: number, digits: number) {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}

export function formatInt(value: number) {
  return integer.format(Math.round(value));
}

export function formatDecimal(value: number, digits: number) {
  let format = fixed.get(digits);
  if (!format) {
    format = new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits });
    fixed.set(digits, format);
  }
  return format.format(value);
}

/**
 * 23 868 → "23,9 k"; por debajo de mil, el número tal cual. Los decimales se eligen después de
 * redondear: 99 960 es "100 k", no "100,0 k", y 999,6 ya es "1,0 k".
 */
export function formatThousands(value: number) {
  if (Math.round(value) < 1000) return formatInt(value);
  const thousands = value / 1000;
  const digits = roundTo(thousands, 1) >= 100 ? 0 : 1;
  return `${formatDecimal(thousands, digits)}${NBSP}k`;
}

export function formatPercent(value: number) {
  return `${formatInt(value)}${NBSP}%`;
}

/** Porcentaje entero de una descarga o instalación: nunca llega a 100 antes de terminar. */
export function progressPercent(percent: number | null, done: boolean) {
  if (done) return 100;
  return Math.min(99, Math.max(0, Math.floor(percent ?? 0)));
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

/** Decimales de un tamaño según su magnitud: 2 por debajo de 10, 1 por debajo de 100 y luego 0. */
function sizeDigits(value: number) {
  return value >= 100 ? 0 : value >= 10 ? 1 : 2;
}

/**
 * Tamaño en unidades binarias. La unidad y los decimales se eligen después de redondear:
 * 1 048 575 B es "1 MiB", no "1024 KiB", y 99,96 KiB es "100 KiB", no "100,0 KiB".
 */
export function formatBytes(bytes: number) {
  if (!(bytes >= 1)) return `0${NBSP}B`;
  const last = BYTE_UNITS.length - 1;
  let exponent = Math.min(Math.max(0, Math.floor(Math.log(bytes) / Math.log(1024))), last);
  let value = bytes / 1024 ** exponent;
  let digits = exponent === 0 ? 0 : sizeDigits(value);
  if (roundTo(value, digits) >= 1024 && exponent < last) {
    exponent += 1;
    value = bytes / 1024 ** exponent;
    digits = sizeDigits(value);
  }
  digits = exponent === 0 ? 0 : sizeDigits(roundTo(value, digits));
  return `${upTo[digits].format(value)}${NBSP}${BYTE_UNITS[exponent]}`;
}

export function formatDate(timestamp: number | null) {
  return timestamp == null ? "—" : dateShort.format(new Date(timestamp));
}

export function formatDateLong(timestamp: number | null) {
  return timestamp == null ? "—" : dateLong.format(new Date(timestamp));
}

export function kdaRatio(kills: number, deaths: number, assists: number, digits = 2) {
  return deaths === 0 ? "Perfecto" : formatDecimal((kills + assists) / deaths, digits);
}
