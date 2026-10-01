// Formato de números, duraciones, tamaños, velocidades y fechas en el idioma de la interfaz (ver
// `formatLocale` en i18n.ts). Cada formateador se crea una vez por locale y se reutiliza.

import { currentLanguage, formatLocale, t } from "./i18n";

const NBSP = " ";
const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB"];
const formatters = new Map<string, Intl.NumberFormat | Intl.DateTimeFormat>();

function cached<T extends Intl.NumberFormat | Intl.DateTimeFormat>(key: string, create: (locale: string) => T): T {
  const locale = formatLocale();
  const id = `${locale}|${key}`;
  let format = formatters.get(id) as T | undefined;
  if (!format) {
    format = create(locale);
    formatters.set(id, format);
  }
  return format;
}

/** Hasta `digits` decimales, sin ceros finales (tamaños). */
function upTo(digits: number) {
  return cached(`up${digits}`, (locale) => new Intl.NumberFormat(locale, { maximumFractionDigits: digits }));
}

function roundTo(value: number, digits: number) {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}

export function formatInt(value: number) {
  return upTo(0).format(Math.round(value));
}

/** Exactamente `digits` decimales (promedios, miles, porcentajes de descarga). */
export function formatDecimal(value: number, digits: number) {
  return cached(
    `fixed${digits}`,
    (locale) => new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }),
  ).format(value);
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

/** El signo como en cada idioma: "20 %" en español (con espacio de no separación), "20%" en inglés. */
function withPercentSign(number: string) {
  return currentLanguage() === "es" ? `${number}${NBSP}%` : `${number}%`;
}

export function formatPercent(value: number) {
  return withPercentSign(formatInt(value));
}

/** Porcentaje entero de una descarga o instalación: nunca llega a 100 antes de terminar. */
export function progressPercent(percent: number | null, done: boolean) {
  if (done) return 100;
  return Math.min(99, Math.max(0, Math.floor(percent ?? 0)));
}

/**
 * Porcentaje de la descarga de un cliente con un decimal: a 6 MB/s cada 1 % de 24 GiB tarda unos
 * 40 s, y un décimo se ve avanzar cada pocos segundos. Se trunca (Intl redondearía 99,96 a
 * "100,0") y no pasa de 99,9 antes de terminar.
 */
export function downloadPercent(percent: number | null, done: boolean) {
  if (done) return 100;
  return Math.min(99.9, Math.max(0, Math.floor((percent ?? 0) * 10) / 10));
}

/** "20,4 %" / "20.4%"; terminado, "100 %". */
export function formatDownloadPercent(value: number) {
  return withPercentSign(value >= 100 ? formatInt(100) : formatDecimal(value, 1));
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
  return `${upTo(digits).format(value)}${NBSP}${BYTE_UNITS[exponent]}`;
}

/**
 * Velocidad en unidades decimales, como la muestran los navegadores: "6,1 MB/s"; por debajo de
 * 1 MB/s, "850 kB/s".
 */
export function formatSpeed(bytesPerSecond: number) {
  if (bytesPerSecond < 999_500) return `${formatInt(bytesPerSecond / 1000)}${NBSP}kB/s`;
  const megabytes = bytesPerSecond / 1_000_000;
  return `${roundTo(megabytes, 1) >= 100 ? formatInt(megabytes) : formatDecimal(megabytes, 1)}${NBSP}MB/s`;
}

/** Tiempo restante redondeado hacia arriba al minuto: "< 1 min", "56 min", "1 h 05 min". */
export function formatEta(seconds: number) {
  const minutes = Math.ceil(seconds / 60);
  const time = seconds < 60
    ? `<${NBSP}1${NBSP}min`
    : minutes < 60
      ? `${minutes}${NBSP}min`
      : `${Math.floor(minutes / 60)}${NBSP}h ${String(minutes % 60).padStart(2, "0")}${NBSP}min`;
  return t("timeLeft", { time });
}

export function formatDate(timestamp: number | null) {
  if (timestamp == null) return "—";
  return cached("dateShort", (locale) => new Intl.DateTimeFormat(locale, {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  })).format(new Date(timestamp));
}

export function formatDateLong(timestamp: number | null) {
  if (timestamp == null) return "—";
  return cached("dateLong", (locale) => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }))
    .format(new Date(timestamp));
}

export function kdaRatio(kills: number, deaths: number, assists: number, digits = 2) {
  return deaths === 0 ? t("perfect") : formatDecimal((kills + assists) / deaths, digits);
}
