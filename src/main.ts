import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { h, hydrateIcons, icon, must, type IconName } from "./dom";
import {
  downloadPercent,
  formatBytes,
  formatClock,
  formatDate,
  formatDateLong,
  formatDecimal,
  formatDownloadPercent,
  formatDuration,
  formatEta,
  formatInt,
  formatPercent,
  formatSpeed,
  formatThousands,
  kdaRatio,
  progressPercent,
} from "./format";
import {
  AVERAGE_ICON_BYTES,
  cachedAssetPath,
  cachedAssetUrl,
  loadCommunityDragonAssets,
  loadDataDragonAssets,
  normalizeKey,
  patchImages,
  type CommunityDragonAssets,
  type DataDragonAssets,
  type MatchAsset,
} from "./game-assets";
import { currentLanguage, plural, setLanguage, systemLanguageGuess, t, translateStatic, type MessageKey } from "./i18n";
import type {
  AppSettings,
  AssetPrefetchProgress,
  AssetPrefetchResult,
  ClientInstallPlan,
  ClientInstallResult,
  ClientRequest,
  ConfirmedPlan,
  ClientWorkMode,
  DownloadProgress,
  Language,
  LibraryPlayer,
  LocalClientStatus,
  PlayerItem,
  PlayerStats,
  ReplayInspection,
  ReplayLibrary,
  ReplayLibraryEntry,
  TeamSummary,
  VanguardState,
} from "./types";

type View = "library" | "clients";
type Tab = "board" | "stats" | "file";
type Filter = "all" | "win" | "loss";
// "unknown": el estado en disco todavía no se leyó (o la carpeta de clientes no está disponible).
type GlyphState = "ready" | "partial" | "missing" | "busy" | "blocked" | "unknown";
// Acentos de color con significado: victoria, derrota, aviso e información. El resto es monocromo.
type Tone = "win" | "loss" | "warn" | "info";
type ChipTone = "solid" | "outline" | Tone;

type InstallRun = {
  gameVersion: string;
  build: string;
  exactBuild: boolean;
  destination: string;
  locale: string;
  /** Repetición desde la que se confirmó y cuántas de la biblioteca usan esa compilación. */
  replayPath: string;
  replayName: string;
  replays: number;
  stage: string;
  detail: string;
  downloaded: number;
  total: number | null;
  percent: number | null;
  done: boolean;
  /** El usuario pulsó Detener: la interfaz ya la muestra detenida mientras el backend la cierra. */
  stopped: boolean;
  /** Bytes descargados en los últimos segundos, para la velocidad y el tiempo restante. */
  samples: Array<{ time: number; bytes: number }>;
  /** Se cumple cuando `install_client` termina, bien o mal. */
  settled: Promise<void> | null;
};

/** Descarga de las imágenes de los parches de la biblioteca. */
type PrefetchRun = {
  /** Parches en orden; en segundo plano se pueden sumar los de una biblioteca nueva. */
  patches: string[];
  /** Parche en curso y cuánto lleva (0–1). */
  index: number;
  fraction: number;
  /** En la pantalla de inicio; si no, en el indicador de la barra superior. */
  foreground: boolean;
  /** Se eligió "Al abrir cada partida", o la red no responde: no se pide nada más. */
  cancelled: boolean;
};

/** Lo que ofrece la pregunta por las imágenes: parches, cuántas imágenes y cuánto pesan. */
type AskImages = { patches: string[]; images: number; bytes: number };

type SplashPhase = "boot" | "wait" | "ask" | "progress" | "hidden";

// Estado de la barra del cliente para la repetición abierta.
type ClientView = {
  glyph: GlyphState;
  text: string;
  /** Velocidad de la descarga en curso, junto a su barra. */
  speed: string;
  /** Color del estado: listo, incompleto o bloqueado, descargando; sin tono, neutro. */
  tone: Tone | null;
  /** Compilación que se usa; `inexact` si es otra del mismo parche. */
  build: string;
  inexact: boolean;
  /** Carpeta del cliente o ejecutable elegido; atenuada si todavía no existe. */
  location: string;
  locationMuted: boolean;
  /** Ruta que abre "Abrir carpeta", si hay algo que mostrar. */
  reveal: string | null;
  /** Acción secundaria (Preparar cliente, Reanudar, Progreso, Elegir carpeta). */
  action: { label: string; icon: IconName; run: () => void; disabled?: boolean } | null;
  executable: string | null;
  /** Porcentaje de la descarga en curso, para la barra junto al estado. */
  progress: number | null;
  /** Idioma que se muestra; sin build no hay cliente al que aplicarlo. */
  locale: string;
  error: string | null;
  /** Aclaración neutra bajo la barra (tras Detener), si no hay un error que mostrar. */
  note: string | null;
};

type ReplayExited = { replayPath: string; exitCode: number | null };

const LAST_LIBRARY_KEY = "chronorift.lastReplayLibrary";
// Parches cuyas imágenes ya se descargaron todas (o la caché recuerda como faltantes).
const PREFETCHED_KEY = "chronorift.prefetchedPatches";
const GAME_EXE = "League of Legends.exe";
// Si el inicio se traba (una unidad de red dormida), la pantalla de inicio no se queda: la
// biblioteca muestra su propia carga.
const SPLASH_BOOT_TIMEOUT_MS = 5_000;
// Espera máxima por los datos de los parches antes de preguntar; los que falten no se cuentan.
const ASK_WAIT_MS = 4_000;
// Ventana de la velocidad de descarga: el promedio de los últimos segundos, no el último evento.
const SPEED_WINDOW_MS = 5_000;
// Con menos tiempo medido la velocidad todavía no dice nada: se muestra al pasar este margen.
const SPEED_MIN_SPAN_MS = 1_000;
// Cada cuánto se repite la última muestra cuando no llegan bytes.
const SPEED_PULSE_MS = 1_000;
// Imágenes de la biblioteca que se decodifican a la vez y cuántas se guardan en memoria.
const WARM_WORKERS = 2;
const WARM_LIMIT = 3_000;
const DATA_DRAGON = "https://ddragon.leagueoflegends.com";
// Plataforma de Riot (del nombre del archivo) → región en las URL de League of Graphs.
const LEAGUE_OF_GRAPHS_REGIONS: Record<string, string> = {
  BR1: "br", EUN1: "eune", EUW1: "euw", JP1: "jp", KR: "kr", LA1: "lan", LA2: "las", ME1: "me",
  NA1: "na", OC1: "oce", PH2: "ph", RU: "ru", SG2: "sg", TH2: "th", TR1: "tr", TW2: "tw", VN2: "vn",
};
const STAGES = ["resolve", "manifest", "plan", "download", "complete"];
// Tono de cada estado del cliente: listo, incompleto, descargando o bloqueado por Vanguard. Sin
// instalar, sin leer o sin build siguen neutros.
const GLYPH_TONES: Partial<Record<GlyphState, Tone>> = { ready: "win", partial: "warn", busy: "info", blocked: "warn" };
const MODE_TONES: Record<ClientWorkMode, Tone> = { install: "info", resume: "warn", repair: "warn", ready: "win" };
const POSITIONS: Record<string, string> = {
  TOP: "TOP",
  JUNGLE: "JG",
  MIDDLE: "MID",
  BOTTOM: "BOT",
  UTILITY: "SUP",
};
const LOCALES: Array<[string, string]> = [
  ["cs_CZ", "Čeština"], ["de_DE", "Deutsch"], ["el_GR", "Ελληνικά"],
  ["en_AU", "English (Australia)"], ["en_GB", "English (UK)"], ["en_PH", "English (Philippines)"],
  ["en_SG", "English (Singapore)"], ["en_US", "English (US)"], ["es_AR", "Español (Argentina)"],
  ["es_ES", "Español (España)"], ["es_MX", "Español (Latinoamérica)"], ["fr_FR", "Français"],
  ["hu_HU", "Magyar"], ["id_ID", "Bahasa Indonesia"], ["it_IT", "Italiano"],
  ["ja_JP", "日本語"], ["ko_KR", "한국어"], ["ms_MY", "Bahasa Melayu"],
  ["pl_PL", "Polski"], ["pt_BR", "Português (Brasil)"], ["ro_RO", "Română"],
  ["ru_RU", "Русский"], ["th_TH", "ไทย"], ["tr_TR", "Türkçe"],
  ["vi_VN", "Tiếng Việt"], ["zh_CN", "简体中文"], ["zh_MY", "中文 (马来西亚)"], ["zh_TW", "繁體中文"],
];

const state = {
  view: "library" as View,
  settings: { clientsRoot: null } as AppSettings,
  library: null as ReplayLibrary | null,
  // Carpeta abierta o, si falló, la que se intentó abrir: Actualizar vuelve a leerla.
  libraryPath: null as string | null,
  libraryBusy: false,
  libraryError: null as string | null,
  owner: null as string | null,
  query: "",
  filter: "all" as Filter,
  selectedPath: null as string | null,
  replay: null as ReplayInspection | null,
  replayLoading: false,
  replayError: null as string | null,
  replayRun: 0,
  // Repeticiones ya leídas por ruta: elegir una de estas es inmediato.
  inspections: new Map<string, ReplayInspection>(),
  libraryRun: 0,
  tab: "board" as Tab,
  statuses: new Map<string, LocalClientStatus>(),
  // Versiones cuyo estado ya se leyó en la carpeta de clientes actual; las demás son desconocidas.
  statusVersions: new Set<string>(),
  // `client_statuses` falló (por ejemplo, una unidad USB desconectada): ningún estado es fiable.
  statusesError: null as string | null,
  installed: [] as LocalClientStatus[],
  manualExecutables: new Map<string, string>(),
  locale: "en_US",
  drawerOpen: false,
  drawerMode: "plan" as "plan" | "progress",
  planKind: "install" as "install" | "resume",
  plan: null as ClientInstallPlan | null,
  // Repetición, idioma y carpeta del plan pedido: solo se reutiliza si coinciden.
  planKey: null as string | null,
  planning: false,
  planStage: null as string | null,
  planError: null as string | null,
  planRun: 0,
  install: null as InstallRun | null,
  // Fallo de la última descarga (o de Detener); se ve hasta el próximo intento.
  installError: null as null | { gameVersion: string; build: string; message: string },
  launch: null as null | { path: string; state: "launching" | "error"; message: string },
  // Partidas abiertas por repetición: inicios menos salidas (evento `replay-exited`).
  playing: new Map<string, number>(),
  vanguard: "unknown" as VanguardState,
  // Última acción fallida sobre la partida abierta: mostrar en carpeta o elegir ejecutable.
  actionError: null as null | { path: string; message: string },
  clientsError: null as string | null,
  dataDragon: null as DataDragonAssets | null,
  communityDragon: null as CommunityDragonAssets | null,
  names: null as DataDragonAssets | null,
  assetRun: 0,
  // Pantalla de inicio: cargando, esperando los datos para preguntar, preguntando, descargando
  // imágenes u oculta. Mientras se ve, el resto de la app queda inerte.
  splash: "boot" as SplashPhase,
  // Primera configuración: si muestra el idioma y qué ofrece la pregunta por las imágenes (sin
  // pregunta, `null`). Solo vale la de la biblioteca más reciente.
  setupLanguage: false,
  ask: null as AskImages | null,
  prefetch: null as PrefetchRun | null,
  // Compilación detenida a pedido en esta sesión: bajo la barra se aclara que Reanudar sigue desde
  // lo descargado, hasta el próximo intento.
  stopNote: null as null | { gameVersion: string },
};

const dataDragonCache = new Map<string, Promise<DataDragonAssets>>();
const communityDragonCache = new Map<string, Promise<CommunityDragonAssets>>();
// Los mismos datos ya resueltos: se asignan sin esperar ni una microtarea, así la partida se
// pinta de una vez con nombres e íconos.
const readyDataDragon = new Map<string, DataDragonAssets>();
const readyCommunityDragon = new Map<string, CommunityDragonAssets>();

type RowRecord = {
  element: HTMLButtonElement;
  glyph: HTMLElement;
  entry: ReplayLibraryEntry | null;
  content: string;
};

// Filas de la biblioteca por ruta: se reutilizan entre renders para que la fila con el foco no se
// reemplace (elegir con Enter no lo pierde).
const replayRows = new Map<string, RowRecord>();
const failureRows = new Map<string, HTMLElement>();
let rowsLibrary: ReplayLibrary | null = null;
// Solo la lectura de estados más reciente se aplica.
let statusRun = 0;
let installedRun = 0;
// Descarga detenida que el backend todavía está cerrando. Un plan o una descarga nueva la esperan:
// si no, el backend los rechazaría con "Ya hay una descarga en curso.".
let stoppedInstall: Promise<void> | null = null;
// Primera partida que elige una biblioteca recién leída: al iniciar, la pantalla de inicio la espera.
let firstSelection: Promise<void> | null = null;
// Pregunta por las imágenes: solo vale la de la biblioteca más reciente.
let askRun = 0;
// Idioma elegido en esta sesión. Manda sobre la configuración aunque no se haya podido guardar, y
// ya no se vuelve a preguntar.
let sessionLanguage: Language | null = null;
// La configuración ya se leyó (o falló): antes no se sabe si el idioma se eligió, y la salida de
// respaldo de la pantalla de inicio no lo pregunta.
let settingsRead = false;
// Llamada a `prefetch_assets` en curso y la descarga a la que pertenece: el backend corre una a la
// vez, y sus avances no dicen de cuál son.
let prefetchCall: Promise<unknown> | null = null;
let prefetchActive: PrefetchRun | null = null;
// Rehace el botón de maximizar (icono y nombre) con el estado de la ventana y el idioma actual.
let syncMaximize: (() => Promise<void>) | null = null;
// Parches ya intentados en esta biblioteca: un fallo de red no se reintenta en bucle, sino al
// volver a leer la carpeta.
const attemptedPatches = new Set<string>();
// Imágenes de la biblioteca ya decodificadas. Mientras sigan aquí, una casilla con la misma URL se
// pinta en el mismo cuadro al cambiar de partida.
const warmImages = new Map<string, HTMLImageElement>();
let warmRun = 0;
let splashExit = 0;

const el = {
  splash: must("#splash", HTMLElement),
  splashDots: must("#splash-dots", HTMLElement),
  splashAsk: must("#splash-ask", HTMLElement),
  setupLanguage: must("#setup-language", HTMLElement),
  setupContinue: must("#setup-continue", HTMLButtonElement),
  setupImages: must("#setup-images", HTMLElement),
  splashAskData: must("#splash-ask-data", HTMLElement),
  languageButtons: [...document.querySelectorAll<HTMLButtonElement>("[data-language]")],
  prefetchAll: must("#prefetch-all", HTMLButtonElement),
  prefetchLater: must("#prefetch-later", HTMLButtonElement),
  splashProgress: must("#splash-progress", HTMLElement),
  splashPercent: must("#splash-percent", HTMLElement),
  splashTrack: must("#splash-track", HTMLElement),
  splashBar: must("#splash-bar", HTMLElement),
  splashPatches: must("#splash-patches", HTMLElement),
  prefetchContinue: must("#prefetch-continue", HTMLButtonElement),
  skipLink: must(".skip-link", HTMLAnchorElement),
  appNav: must(".appbar__nav", HTMLElement),
  appEnd: must(".appbar__end", HTMLElement),
  navItems: [...document.querySelectorAll<HTMLButtonElement>(".nav-item")],
  activity: must("#activity", HTMLButtonElement),
  activityLabel: must("#activity-label", HTMLElement),
  activityBuild: must("#activity-build", HTMLElement),
  activityBar: must("#activity-bar", HTMLElement),
  activityPercent: must("#activity-percent", HTMLElement),
  activitySpeed: must("#activity-speed", HTMLElement),
  storageChip: must("#storage-chip", HTMLButtonElement),
  storageRoot: must("#storage-root", HTMLElement),
  libraryView: must("#library-view", HTMLElement),
  clientsView: must("#clients-view", HTMLElement),
  replayCount: must("#replay-count", HTMLElement),
  refreshLibrary: must("#refresh-library", HTMLButtonElement),
  openLibrary: must("#open-library", HTMLButtonElement),
  folderPath: must("#folder-path", HTMLElement),
  search: must("#replay-search", HTMLInputElement),
  resultFilter: must("#result-filter", HTMLElement),
  libraryList: must("#library-list", HTMLElement),
  libraryEmpty: must("#library-empty", HTMLElement),
  libraryEmptyText: must("#library-empty-text", HTMLElement),
  libraryOwner: must("#library-owner", HTMLElement),
  libraryOwnerName: must("#library-owner-name", HTMLElement),
  librarySummary: must("#library-summary", HTMLElement),
  matchMain: must("#main-content", HTMLElement),
  replayEmpty: must("#replay-empty", HTMLElement),
  emptyTitle: must("#empty-title", HTMLElement),
  emptyDetail: must("#empty-detail", HTMLElement),
  workspace: must("#replay-workspace", HTMLElement),
  fileName: must("#file-name", HTMLElement),
  fileSize: must("#file-size", HTMLElement),
  resultChip: must("#result-chip", HTMLElement),
  resultSide: must("#result-side", HTMLElement),
  score: must("#score", HTMLElement),
  blueScore: must("#blue-score", HTMLElement),
  redScore: must("#red-score", HTMLElement),
  matchDate: must("#match-date", HTMLElement),
  duration: must("#duration", HTMLElement),
  patch: must("#patch", HTMLElement),
  region: must("#region", HTMLElement),
  matchId: must("#match-id", HTMLButtonElement),
  copyMatchId: must("#copy-match-id", HTMLButtonElement),
  version: must("#version", HTMLElement),
  versionInexact: must("#version-inexact", HTMLElement),
  chooseExecutable: must("#choose-executable", HTMLButtonElement),
  clientAction: must("#client-action", HTMLButtonElement),
  clientActionIcon: must("#client-action-icon", HTMLElement),
  clientActionLabel: must("#client-action-label", HTMLElement),
  play: must("#play-replay", HTMLButtonElement),
  clientStatus: must("#client-status", HTMLElement),
  clientGlyph: must("#client-glyph", HTMLElement),
  clientText: must("#client-status-text", HTMLElement),
  clientLocation: must("#client-location", HTMLElement),
  revealClient: must("#reveal-client", HTMLButtonElement),
  clientLocale: must("#client-locale", HTMLElement),
  clientProgress: must("#client-progress", HTMLElement),
  clientProgressBar: must("#client-progress-bar", HTMLElement),
  clientSpeed: must("#client-speed", HTMLElement),
  clientError: must("#client-error", HTMLElement),
  clientNote: must("#client-note", HTMLElement),
  tabs: [...document.querySelectorAll<HTMLButtonElement>(".tab")],
  panelBoard: must("#panel-board", HTMLElement),
  boardHead: must("#panel-board .sb-row--head", HTMLElement),
  panelStats: must("#panel-stats", HTMLElement),
  panelFile: must("#panel-file", HTMLElement),
  teams: must("#teams", HTMLElement),
  statsEmpty: must("#stats-empty", HTMLElement),
  statsTop: must("#stats-top", HTMLElement),
  statsPlayers: must("#stats-players", HTMLElement),
  teamCompare: must("#team-compare", HTMLElement),
  damageChart: must("#damage-chart", HTMLElement),
  playerTable: must("#player-table", HTMLElement),
  fileFacts: must("#file-facts", HTMLElement),
  drawer: must("#client-drawer", HTMLElement),
  drawerClose: must("#drawer-close", HTMLButtonElement),
  scrim: must("#drawer-scrim", HTMLElement),
  drawerTitle: must("#drawer-title", HTMLElement),
  drawerSubtitle: must("#drawer-subtitle", HTMLElement),
  planMode: must("#plan-mode", HTMLElement),
  planVersion: must("#plan-version", HTMLElement),
  planView: must("#plan-view", HTMLElement),
  planDestination: must("#plan-destination", HTMLElement),
  planChangeRoot: must("#plan-change-root", HTMLButtonElement),
  locale: must("#locale", HTMLSelectElement),
  planFileCount: must("#plan-file-count", HTMLElement),
  planDownloadSize: must("#plan-download-size", HTMLElement),
  planInstallSize: must("#plan-install-size", HTMLElement),
  planSource: must("#plan-source", HTMLElement),
  planError: must("#plan-error", HTMLElement),
  progressView: must("#progress-view", HTMLElement),
  progressPercent: must("#progress-percent", HTMLElement),
  progressBytes: must("#progress-bytes", HTMLElement),
  progressTrack: must("#progress-track", HTMLElement),
  progressBar: must("#progress-bar", HTMLElement),
  progressRate: must("#progress-rate", HTMLElement),
  progressFile: must("#progress-file", HTMLElement),
  stages: [...document.querySelectorAll<HTMLElement>("#stages li")],
  progressDestination: must("#progress-destination", HTMLElement),
  progressLocale: must("#progress-locale", HTMLElement),
  cancelClient: must("#cancel-client-action", HTMLButtonElement),
  confirmClient: must("#confirm-client-action", HTMLButtonElement),
  confirmIcon: must("#confirm-icon", HTMLElement),
  confirmLabel: must("#confirm-label", HTMLElement),
  stopClient: must("#stop-client-action", HTMLButtonElement),
  clientsMain: must("#clients-content", HTMLElement),
  installFolder: must("#install-folder", HTMLInputElement),
  revealClientsRoot: must("#reveal-clients-root", HTMLButtonElement),
  clientsRootError: must("#clients-root-error", HTMLElement),
  assetPrefetch: must("#asset-prefetch", HTMLElement),
  clientsInstalled: must("#clients-installed", HTMLElement),
  clientsBytes: must("#clients-bytes", HTMLElement),
  buildsCount: must("#builds-count", HTMLElement),
  buildsList: must("#builds-list", HTMLElement),
};

// La pantalla de inicio de index.html ya se ve: hasta que salga, la app de detrás no recibe el foco.
setAppInert(true);
hydrateIcons();
initializeLocales();
// Hasta leer la configuración, el idioma del sistema según el visor.
applyLanguage(systemLanguageGuess());
bindEvents();
renderAll();
if (import.meta.env.PROD) blockReloads();
if ("__TAURI_INTERNALS__" in window) {
  void listen<DownloadProgress>("download-progress", ({ payload }) => onProgress(payload));
  void listen<AssetPrefetchProgress>("asset-prefetch-progress", ({ payload }) => onPrefetchProgress(payload));
  void listen<ReplayExited>("replay-exited", ({ payload }) => onReplayExited(payload));
  window.addEventListener("focus", onWindowFocus);
  bindWindowControls();
  void boot();
} else {
  // Sin backend (una vista previa en el navegador) no hay nada que esperar.
  hideSplash();
}

/**
 * Recargar la vista pierde todo el estado: una descarga en curso seguiría sin interfaz y se podría
 * confirmar otra. En la app instalada se anulan F5, Ctrl+R y Ctrl+Mayús+R, y el menú contextual
 * del visor (que ofrece "Actualizar") salvo en los campos de texto, donde sirve para copiar y pegar.
 */
function blockReloads() {
  window.addEventListener(
    "keydown",
    (event) => {
      const key = event.key.toLowerCase();
      if (key === "f5" || ((event.ctrlKey || event.metaKey) && key === "r")) event.preventDefault();
    },
    true,
  );
  window.addEventListener("contextmenu", (event) => {
    const target = event.target;
    const editable = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
    if (!editable) event.preventDefault();
  });
}

function onWindowFocus() {
  // Vanguard se detiene desde su icono de bandeja; al volver a la ventana se relee su estado.
  void refreshVanguard();
  // Una unidad de clientes desconectada puede haber vuelto mientras la ventana no tenía el foco.
  if (state.statusesError) void refreshClientStatuses();
}

// La ventana no tiene barra de título nativa: la barra superior la reemplaza.
function bindWindowControls() {
  let current: ReturnType<typeof getCurrentWindow>;
  try {
    current = getCurrentWindow();
  } catch {
    return;
  }
  const maximize = must("#window-maximize", HTMLButtonElement);
  must("#window-minimize", HTMLButtonElement).addEventListener("click", () => void current.minimize());
  maximize.addEventListener("click", () => void current.toggleMaximize());
  must("#window-close", HTMLButtonElement).addEventListener("click", () => void current.close());
  const sync = async () => {
    try {
      const maximized = await current.isMaximized();
      const label = t(maximized ? "restore" : "maximize");
      maximize.replaceChildren(icon(maximized ? "restore" : "maximize"));
      maximize.setAttribute("aria-label", label);
      maximize.title = label;
    } catch {
      // Sin estado de la ventana se conserva el icono actual.
    }
  };
  syncMaximize = sync;
  void sync();
  current.onResized(() => void sync()).catch(() => undefined);
}

async function boot() {
  // Respaldo: si la configuración o la carpeta tardan demasiado, la app se muestra igual.
  const backup = window.setTimeout(() => {
    if (state.splash === "boot") leaveSplash();
  }, SPLASH_BOOT_TIMEOUT_MS);
  try {
    void refreshVanguard();
    await loadSettings();
    const saved = localStorage.getItem(LAST_LIBRARY_KEY);
    if (saved) {
      await loadLibrary(saved);
      // La partida elegida, con los nombres de su parche: sin "Cargando…" ni nombres que llegan
      // después. Las imágenes no se esperan; aparecen con su fundido.
      await firstSelection;
      if (state.replay?.patch) await loadPatchAssets(state.replay.patch);
    }
  } finally {
    window.clearTimeout(backup);
    // Biblioteca cargada, fallida o sin carpeta recordada: sale la pantalla de inicio, salvo que
    // esté preguntando por las imágenes o falte elegir el idioma.
    if (state.splash === "boot") leaveSplash();
  }
}

// ——— Pantalla de inicio ———

/** Con la pantalla de inicio a la vista, la app de detrás queda fuera del foco; los botones de la
 *  ventana siguen disponibles. */
function setAppInert(inert: boolean) {
  for (const part of [el.skipLink, el.appNav, el.appEnd, el.libraryView, el.clientsView]) part.inert = inert;
}

function showSplash(phase: Exclude<SplashPhase, "boot" | "hidden">) {
  state.splash = phase;
  setAppInert(true);
  // Anula una salida en curso; si ya se había ocultado, vuelve con un fundido.
  splashExit++;
  if (el.splash.hidden) {
    el.splash.hidden = false;
    el.splash.classList.add("is-leaving");
    void el.splash.offsetWidth;
  }
  el.splash.classList.remove("is-leaving");
  renderSplash();
}

/** Sale la pantalla de inicio; si falta elegir el idioma, antes pregunta solo eso. */
function leaveSplash() {
  if (needsLanguageQuestion()) showSetup(null);
  else hideSplash();
}

/** El idioma nunca se eligió (ni en esta sesión): la primera configuración lo pregunta. */
function needsLanguageQuestion() {
  return settingsRead && sessionLanguage == null && state.settings.language == null;
}

/**
 * Primera configuración en la pantalla de inicio: el idioma, si falta elegirlo, y la pregunta por
 * las imágenes, si hay una. Solo con el idioma, "Continuar" la cierra; con las imágenes, cualquiera
 * de sus dos respuestas.
 */
function showSetup(ask: AskImages | null) {
  state.ask = ask;
  state.setupLanguage = needsLanguageQuestion();
  showSplash("ask");
  (ask ? el.prefetchAll : el.setupContinue).focus();
}

/** Al terminar la primera configuración se guarda el idioma a la vista, aunque no se tocara. */
function confirmSetupLanguage() {
  if (needsLanguageQuestion()) void chooseLanguage(currentLanguage());
}

/** Sale con un fundido breve; sin transición en curso se oculta en el acto. */
function hideSplash() {
  if (state.splash === "hidden") return;
  const focused = el.splash.contains(document.activeElement);
  state.splash = "hidden";
  state.ask = null;
  state.setupLanguage = false;
  setAppInert(false);
  const exit = ++splashExit;
  el.splash.classList.add("is-leaving");
  if (focused) (document.activeElement as HTMLElement | null)?.blur();
  // Lo que mostraba se conserva durante el fundido y se retira al terminar.
  const finish = () => {
    if (exit !== splashExit) return;
    el.splash.hidden = true;
    renderSplash();
  };
  const running = el.splash.getAnimations();
  if (running.length === 0) finish();
  else void Promise.allSettled(running.map((animation) => animation.finished)).then(finish);
  renderAppBar();
  // La biblioteca ya está a la vista: se preparan sus imágenes para cambiar de partida al instante.
  void warmLibraryImages();
}

function renderSplash() {
  const phase = state.splash;
  el.splashDots.hidden = phase !== "boot" && phase !== "wait";
  el.splashAsk.hidden = phase !== "ask";
  el.splashProgress.hidden = phase !== "progress";
  if (phase === "ask") {
    const ask = state.ask;
    el.setupLanguage.hidden = !state.setupLanguage;
    el.setupImages.hidden = !ask;
    el.setupContinue.hidden = !!ask;
    setAttribute(el.splashAsk, "aria-labelledby", ask ? "splash-ask-title" : "setup-language-label");
    if (ask) setText(el.splashAskData, askText(ask));
  }
  const run = state.prefetch;
  if (phase !== "progress" || !run) return;
  const percent = prefetchPercent(run);
  setText(el.splashPercent, formatPercent(percent));
  el.splashBar.style.width = `${percent}%`;
  el.splashTrack.setAttribute("aria-valuenow", String(percent));
  setText(el.splashPatches, t("patchesDone", { done: Math.min(run.index, run.patches.length), total: run.patches.length }));
}

/** "2 parches · 2326 imágenes · ~24,5 MiB", en el idioma actual. */
function askText(ask: AskImages) {
  return t("askImagesData", {
    patches: plural(ask.patches.length, "patchesOne", "patchesOther", formatInt(ask.patches.length)),
    images: plural(ask.images, "imagesOne", "imagesOther", formatInt(ask.images)),
    size: formatBytes(ask.bytes),
  });
}

// ——— Idioma ———

/** Idioma de la interfaz: el elegido en esta sesión, el guardado o, si nunca se eligió, el del sistema. */
function effectiveLanguage(): Language {
  return sessionLanguage ?? state.settings.language ?? state.settings.systemLanguage ?? systemLanguageGuess();
}

/**
 * Cambia el idioma en el acto, sin recargar: textos fijos y pintados, formatos, el botón de
 * maximizar y los nombres de campeones, objetos y runas de la partida abierta y de la lista.
 */
function applyLanguage(language: Language) {
  const changed = language !== currentLanguage();
  setLanguage(language);
  translateStatic();
  for (const button of el.languageButtons) setAttribute(button, "aria-pressed", String(button.dataset.language === language));
  if (!changed) return;
  void syncMaximize?.();
  reloadLocalizedData();
  renderAll();
  renderSplash();
}

/** Elige el idioma: se ve en el acto y se guarda. Si no se puede guardar, vale para esta sesión. */
async function chooseLanguage(language: Language) {
  sessionLanguage = language;
  applyLanguage(language);
  try {
    const saved = await invoke<AppSettings>("set_language", { language });
    // Solo el idioma: otra elección guardada a la vez (las imágenes) puede volver en otro orden.
    state.settings = { ...state.settings, language: saved.language, systemLanguage: saved.systemLanguage };
  } catch {
    // Sin guardar, la elección sigue en memoria (`sessionLanguage`).
    return;
  }
  rereadReplayNotes();
}

/**
 * Las notas de una repetición (por qué no tiene estadísticas) llegan del backend en su idioma: las
 * leídas antes del cambio se vuelven a leer, la de la partida abierta en el acto y las demás al
 * elegirlas.
 */
function rereadReplayNotes() {
  for (const [path, replay] of state.inspections) if (replay.notes.length) state.inspections.delete(path);
  const replay = state.replay;
  if (!replay?.notes.length) return;
  const generation = state.libraryRun;
  invoke<ReplayInspection>("inspect_replay", { path: replay.path }).then(
    (fresh) => {
      if (generation === state.libraryRun) state.inspections.set(fresh.path, fresh);
      if (state.replay?.path === fresh.path) showReplay(fresh);
    },
    () => undefined,
  );
}

/**
 * Los datos de DataDragon y CommunityDragon se piden por idioma (sus claves lo incluyen): la partida
 * abierta y la lista los vuelven a leer. Mientras llegan siguen los nombres anteriores; las
 * imágenes no dependen del idioma.
 */
function reloadLocalizedData() {
  const replay = state.replay;
  if (replay) {
    const ready = readyAssets(replay);
    if (ready.data && ready.community) {
      state.dataDragon = ready.data;
      state.communityDragon = ready.community;
    } else {
      void enrichMatch(replay, true);
    }
  }
  if (state.library) {
    warmPatchData(state.library);
    void loadChampionNames(state.library);
  }
}

// ——— Eventos ———

function bindEvents() {
  // El contenido principal depende de la vista: la partida o la lista de clientes.
  el.skipLink.addEventListener("click", (event) => {
    event.preventDefault();
    (state.view === "clients" ? el.clientsMain : el.matchMain).focus();
  });
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-view]")) {
    button.addEventListener("click", () => setView(button.dataset.view as View));
  }
  // El indicador lleva a la descarga del cliente o, si son las imágenes, a su ajuste en Clientes.
  el.activity.addEventListener("click", () => {
    if (state.install) showProgress();
    else if (state.prefetch) setView("clients");
  });
  el.prefetchAll.addEventListener("click", () => answerPrefetch(true));
  el.prefetchLater.addEventListener("click", () => answerPrefetch(false));
  el.prefetchContinue.addEventListener("click", continuePrefetchInBackground);
  // El mismo control en la primera configuración y en Clientes.
  for (const button of el.languageButtons) {
    button.addEventListener("click", () => {
      const language = button.dataset.language as Language;
      if (language !== currentLanguage() || needsLanguageQuestion()) void chooseLanguage(language);
    });
  }
  el.setupContinue.addEventListener("click", () => {
    if (state.splash !== "ask" || state.ask) return;
    confirmSetupLanguage();
    hideSplash();
  });
  for (const button of el.assetPrefetch.querySelectorAll<HTMLButtonElement>("[data-prefetch]")) {
    button.addEventListener("click", () => void chooseAssetPrefetch(button.dataset.prefetch === "all"));
  }
  for (const button of document.querySelectorAll<HTMLButtonElement>("#open-library, [data-action='open-library']")) {
    button.addEventListener("click", () => void chooseLibrary());
  }
  // También reintenta una carpeta que no se pudo leer (una unidad de red dormida, un USB).
  el.refreshLibrary.addEventListener("click", () => {
    if (state.libraryPath) void loadLibrary(state.libraryPath);
  });
  el.search.addEventListener("input", () => {
    state.query = el.search.value;
    renderLibrary();
  });
  for (const button of el.resultFilter.querySelectorAll<HTMLButtonElement>("[data-filter]")) {
    button.addEventListener("click", () => {
      state.filter = button.dataset.filter as Filter;
      renderLibrary();
    });
  }
  for (const tab of el.tabs) {
    tab.addEventListener("click", () => {
      state.tab = tab.dataset.tab as Tab;
      renderMatch();
    });
  }
  el.copyMatchId.addEventListener("click", () => {
    if (state.replay?.matchId) void copyText(state.replay.matchId, el.copyMatchId);
  });
  el.matchId.addEventListener("click", () => {
    const url = matchPageUrl(state.replay);
    // El permiso solo admite páginas de partida de League of Graphs; un fallo no afecta a la app.
    if (url) openUrl(url).catch(() => undefined);
  });
  const copyPath = must("#copy-path", HTMLButtonElement);
  copyPath.addEventListener("click", () => {
    if (state.replay) void copyText(state.replay.path, copyPath);
  });
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-action='reveal-replay']")) {
    button.addEventListener("click", () => {
      if (state.replay) void reveal(state.replay.path, "match");
    });
  }
  el.chooseExecutable.addEventListener("click", () => void chooseExecutable());
  el.clientAction.addEventListener("click", () => clientView()?.action?.run());
  el.revealClient.addEventListener("click", () => {
    const path = clientView()?.reveal;
    if (path) void reveal(path, "match");
  });
  el.play.addEventListener("click", () => void playReplay());
  el.scrim.addEventListener("click", closeDrawer);
  el.drawerClose.addEventListener("click", closeDrawer);
  // Escape cierra el panel aunque el foco esté en la biblioteca, que sigue disponible al lado.
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !event.defaultPrevented && state.drawerOpen && state.view === "library") {
      closeDrawer();
    }
  });
  el.cancelClient.addEventListener("click", closeDrawer);
  el.confirmClient.addEventListener("click", () => void confirmInstall());
  el.stopClient.addEventListener("click", () => void stopInstall());
  // Guardar otra carpeta vuelve a planificar el panel abierto.
  el.planChangeRoot.addEventListener("click", () => void chooseClientsRoot());
  el.locale.addEventListener("change", () => {
    state.locale = el.locale.value;
    renderClientStrip();
    if (state.drawerOpen && state.drawerMode === "plan") void refreshPlan();
  });
  el.installFolder.addEventListener("change", () => void saveClientsRoot(el.installFolder.value));
  must("#choose-install-folder", HTMLButtonElement).addEventListener("click", () => void chooseClientsRoot());
  el.revealClientsRoot.addEventListener("click", () => {
    if (state.settings.clientsRoot) void reveal(state.settings.clientsRoot, "clients");
  });
}

function setView(view: View) {
  state.view = view;
  if (view === "clients") void refreshInstalledClients();
  renderAll();
}

// ——— Configuración ———

async function loadSettings() {
  try {
    state.settings = await invoke<AppSettings>("get_settings");
    state.clientsError = null;
  } catch (error) {
    state.clientsError = errorText(error);
  }
  settingsRead = true;
  applyLanguage(effectiveLanguage());
  renderAll();
}

// El backend normaliza la ruta (una raíz de unidad pasa a <unidad>\ChronoRift) y no crea nada.
async function saveClientsRoot(path: string) {
  if (!path.trim()) return false;
  try {
    const previous = state.settings.clientsRoot;
    state.settings = await invoke<AppSettings>("set_clients_root", { path });
    state.clientsError = null;
    if (state.settings.clientsRoot !== previous) {
      // Lo leído, lo planificado y el fallo de la última descarga eran de la carpeta anterior.
      state.statuses = new Map();
      state.statusVersions = new Set();
      state.statusesError = null;
      if (!state.install) state.installError = null;
      if (state.drawerOpen && state.drawerMode === "plan") void refreshPlan();
      else dropPlan();
    }
    await refreshClientStatuses();
    return true;
  } catch (error) {
    state.clientsError = errorText(error);
    return false;
  } finally {
    renderAll();
  }
}

async function chooseClientsRoot() {
  let selected: string | string[] | null;
  try {
    selected = await open({ directory: true, multiple: false, defaultPath: state.settings.clientsRoot ?? undefined });
  } catch (error) {
    state.clientsError = errorText(error);
    renderAll();
    return false;
  }
  return typeof selected === "string" ? saveClientsRoot(selected) : false;
}

// ——— Biblioteca ———

async function chooseLibrary() {
  if (state.libraryBusy) return;
  let selected: string | string[] | null;
  try {
    selected = await open({ directory: true, multiple: false, defaultPath: state.libraryPath ?? undefined });
  } catch {
    return; // Sin diálogo no hay carpeta nueva; la biblioteca actual sigue igual.
  }
  if (typeof selected === "string") await loadLibrary(selected);
}

async function loadLibrary(path: string) {
  if (state.libraryBusy) return;
  state.libraryBusy = true;
  renderLibrary();
  try {
    const library = await invoke<ReplayLibrary>("inspect_replay_folder", { path });
    library.entries.sort(compareEntries);
    state.library = library;
    state.libraryPath = library.folderPath;
    state.libraryError = null;
    // Un escaneo nuevo reemplaza lo leído antes: los archivos pudieron cambiar.
    state.inspections.clear();
    void prefetchLibrary(library);
    // Antes de mostrarla: la primera vez se pregunta por las imágenes; después, las de un parche
    // nuevo se descargan en segundo plano si así se eligió.
    attemptedPatches.clear();
    if (state.settings.assetPrefetch == null) void askPrefetch(library);
    else prefetchLibraryPatches();
    state.owner = detectOwner(library.entries);
    if (!state.owner) state.filter = "all";
    localStorage.setItem(LAST_LIBRARY_KEY, library.folderPath);
    void refreshClientStatuses();
    void loadChampionNames(library);
    // La selección vigente al terminar el escaneo, no la del momento del clic: pudo cambiar.
    const target = library.entries.find((entry) => entry.path === state.selectedPath)
      ?? visibleEntries()[0]
      ?? library.entries[0];
    if (target) firstSelection = selectReplay(target.path);
    else clearSelection();
  } catch (error) {
    state.library = null;
    state.libraryPath = path;
    state.owner = null;
    state.libraryError = errorText(error);
    // La lectura previa de la carpeta anterior deja de servir, y también su pregunta pendiente.
    state.libraryRun++;
    state.inspections.clear();
    clearSelection();
    cancelAsk();
    // La carpeta recordada se conserva: el backend no distingue una carpeta borrada de una unidad
    // de red dormida o un USB desconectado, que vuelven. Se reemplaza al abrir otra que funcione.
  } finally {
    state.libraryBusy = false;
    renderAll();
  }
}

/** Sin repetición elegida: una lectura que siga pendiente ya no se mostrará al llegar. */
function clearSelection() {
  state.replayRun++;
  state.selectedPath = null;
  state.replay = null;
  state.replayLoading = false;
  state.replayError = null;
  state.launch = null;
  state.actionError = null;
  if (!state.install) {
    state.drawerOpen = false;
    dropPlan();
  }
}

/** La más reciente primero por fecha del archivo; a igual fecha, el ID de partida mayor. */
function compareEntries(left: ReplayLibraryEntry, right: ReplayLibraryEntry) {
  const leftTime = left.modifiedAtMs ?? Number.NEGATIVE_INFINITY;
  const rightTime = right.modifiedAtMs ?? Number.NEGATIVE_INFINITY;
  if (leftTime !== rightTime) return rightTime > leftTime ? 1 : -1;
  return compareMatchIds(right.matchId, left.matchId) || left.fileName.localeCompare(right.fileName);
}

/** Orden ascendente de IDs de partida numéricos; sin ID va primero. */
function compareMatchIds(left: string | null, right: string | null) {
  if (left === right) return 0;
  if (left == null) return -1;
  if (right == null) return 1;
  if (/^\d+$/.test(left) && /^\d+$/.test(right) && left.length !== right.length) {
    return left.length - right.length;
  }
  return left < right ? -1 : left > right ? 1 : 0;
}

/** El dueño de la biblioteca es el Riot ID presente en todas las repeticiones analizadas. */
function detectOwner(entries: ReplayLibraryEntry[]) {
  const parsed = entries.filter((entry) => entry.players.length > 0);
  if (parsed.length < 2) return null;
  const counts = new Map<string, number>();
  for (const entry of parsed) {
    for (const name of new Set(entry.players.map((player) => player.name).filter(Boolean))) {
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
  }
  const everywhere = [...counts].filter(([, count]) => count === parsed.length).map(([name]) => name);
  return everywhere.length === 1 ? everywhere[0] : null;
}

function ownerOf(entry: ReplayLibraryEntry): LibraryPlayer | null {
  return state.owner ? entry.players.find((player) => player.name === state.owner) ?? null : null;
}

function visibleEntries() {
  const query = state.query.trim().toLocaleLowerCase();
  return (state.library?.entries ?? []).filter((entry) => {
    const owner = ownerOf(entry);
    if (state.filter !== "all" && owner && owner.win !== (state.filter === "win")) return false;
    if (state.filter !== "all" && !owner) return false;
    if (!query) return true;
    const haystack = [
      entry.fileName,
      entry.patch,
      entry.gameVersion,
      entry.region,
      entry.matchId,
      owner?.champion,
      owner ? libraryChampionName(owner.champion) : null,
    ].filter(Boolean).join(" ").toLocaleLowerCase();
    return haystack.includes(query);
  });
}

/** Archivos que el escaneo no pudo leer: solo con "Todas" y se buscan por su nombre. */
function visibleFailures() {
  if (state.filter !== "all") return [];
  const query = state.query.trim().toLocaleLowerCase();
  return (state.library?.failures ?? []).filter(
    (failure) => !query || fileNameOf(failure.path).toLocaleLowerCase().includes(query),
  );
}

async function refreshClientStatuses() {
  const run = ++statusRun;
  const root = state.settings.clientsRoot;
  const versions = [...new Set((state.library?.entries ?? []).map((entry) => entry.gameVersion).filter(Boolean))] as string[];
  if (!root || !versions.length) {
    // Nada que leer: tampoco queda un error de una lectura anterior.
    state.statuses = new Map();
    state.statusVersions = new Set();
    state.statusesError = null;
  } else {
    try {
      const statuses = await invoke<Record<string, LocalClientStatus>>("client_statuses", { clientsRoot: root, versions });
      if (run !== statusRun) return; // Otra lectura más nueva manda.
      state.statuses = new Map(Object.entries(statuses));
      // Una versión que el backend no devuelve no tiene cliente: se conoce como no instalada.
      state.statusVersions = new Set(versions);
      state.statusesError = null;
      state.clientsError = null;
    } catch (error) {
      if (run !== statusRun) return;
      // Sin lectura (por ejemplo, un USB desconectado) no queda ningún "Listo" ni "No instalado" viejo.
      state.statuses = new Map();
      state.statusVersions = new Set();
      state.statusesError = errorText(error);
    }
  }
  if (state.view === "clients") await refreshInstalledClients();
  renderAll();
}

async function refreshInstalledClients() {
  const run = ++installedRun;
  const root = state.settings.clientsRoot;
  if (!root) {
    state.installed = [];
  } else {
    try {
      const installed = await invoke<LocalClientStatus[]>("list_clients", { clientsRoot: root });
      if (run !== installedRun) return;
      state.installed = installed;
      state.clientsError = null;
    } catch (error) {
      if (run !== installedRun) return;
      state.installed = [];
      state.clientsError = errorText(error);
    }
  }
  renderClients();
}

async function loadChampionNames(library: ReplayLibrary) {
  const patches = library.entries
    .map((entry) => entry.patch)
    .filter((patch): patch is string => !!patch)
    .sort(compareVersions);
  const newest = patches[patches.length - 1];
  if (!newest) return;
  try {
    state.names = await dataDragon(`${newest}.1`);
    renderLibrary();
  } catch {
    // Sin nombres localizados se muestran los identificadores de Riot.
  }
}

async function selectReplay(path: string) {
  const run = ++state.replayRun;
  const generation = state.libraryRun;
  state.selectedPath = path;
  const known = state.inspections.get(path);
  if (known) {
    showReplay(known);
    return;
  }
  // Todavía sin leer: la partida anterior sigue a la vista hasta tener la nueva.
  state.replayLoading = true;
  state.replayError = null;
  renderLibrary();
  renderMatch();
  try {
    const replay = await invoke<ReplayInspection>("inspect_replay", { path });
    // Lo leído antes de otro escaneo no entra en la caché del nuevo.
    if (generation === state.libraryRun) state.inspections.set(path, replay);
    if (run === state.replayRun) showReplay(replay);
  } catch (error) {
    if (run !== state.replayRun) return;
    clearSelection();
    state.selectedPath = path;
    state.replayError = errorText(error);
    renderAll();
  }
}

/** Muestra una repetición ya leída; con los datos de su parche en memoria, todo sale de una vez. */
function showReplay(replay: ReplayInspection) {
  // Releer la misma (tras Actualizar) conserva el idioma elegido, el plan y el panel abiertos.
  const same = state.replay?.path === replay.path;
  state.replay = replay;
  state.replayLoading = false;
  state.replayError = null;
  if (!same) {
    state.locale = replay.suggestedLocale ?? "en_US";
    state.launch = null;
    state.actionError = null;
    if (!state.install) state.drawerOpen = false;
    dropPlan();
  }
  const ready = readyAssets(replay);
  // La misma partida releída (otro idioma) conserva los nombres que ya tenía hasta que lleguen los
  // nuevos: sin un cuadro con los identificadores de Riot.
  state.dataDragon = ready.data ?? (same ? state.dataDragon : null);
  state.communityDragon = ready.community ?? (same ? state.communityDragon : null);
  renderAll();
  if (!ready.data || !ready.community) void enrichMatch(replay, same);
}

/**
 * Lee de antemano cada repetición de la biblioteca y los datos de cada parche, en el orden de la
 * lista, para que elegir una partida no tenga que esperar nada.
 */
async function prefetchLibrary(library: ReplayLibrary) {
  const run = ++state.libraryRun;
  warmPatchData(library);
  const queue = [...library.entries];
  const worker = async () => {
    for (let entry = queue.shift(); entry && run === state.libraryRun; entry = queue.shift()) {
      if (state.inspections.has(entry.path)) continue;
      try {
        const replay = await invoke<ReplayInspection>("inspect_replay", { path: entry.path });
        if (run === state.libraryRun) state.inspections.set(entry.path, replay);
      } catch {
        // Se vuelve a intentar al elegirla, y ahí se muestra el error.
      }
    }
  };
  await Promise.all([worker(), worker(), worker()]);
  if (run === state.libraryRun) void warmLibraryImages();
}

/** Pide de antemano los datos de cada parche de la biblioteca, en el idioma actual. */
function warmPatchData(library: ReplayLibrary) {
  const patches = new Set(library.entries.map((entry) => entry.patch).filter((patch): patch is string => !!patch));
  for (const patch of patches) {
    dataDragon(`${patch}.1`).catch(() => undefined);
    communityDragon(patch).catch(() => undefined);
  }
}

/**
 * Idioma de los nombres de campeones, objetos y runas: el de la interfaz. En español, el de España
 * con un sistema en es-ES y el de Latinoamérica con cualquier otro.
 */
function namesLocale() {
  if (currentLanguage() === "en") return "en_US";
  return typeof navigator !== "undefined" && navigator.language?.toLowerCase() === "es-es" ? "es_ES" : "es_MX";
}

function assetKeys(replay: ReplayInspection) {
  const locale = namesLocale();
  return {
    data: `${replay.dataDragonVersion ?? `${replay.patch}.1`}:${locale}`,
    community: `${replay.patch}:${locale}`,
  };
}

function readyAssets(replay: ReplayInspection) {
  if (replay.status !== "parsed" || !replay.patch) return { data: null, community: null };
  const keys = assetKeys(replay);
  return {
    data: readyDataDragon.get(keys.data) ?? null,
    community: readyCommunityDragon.get(keys.community) ?? null,
  };
}

function dataDragon(version: string) {
  const locale = namesLocale();
  const key = `${version}:${locale}`;
  let entry = dataDragonCache.get(key);
  if (!entry) {
    entry = loadDataDragonAssets(version, locale);
    dataDragonCache.set(key, entry);
    // Un fallo de red no queda en caché: el próximo intento vuelve a consultar.
    entry.then(
      (assets) => readyDataDragon.set(key, assets),
      () => dataDragonCache.delete(key),
    );
  }
  return entry;
}

function communityDragon(patch: string) {
  const locale = namesLocale();
  const key = `${patch}:${locale}`;
  let entry = communityDragonCache.get(key);
  if (!entry) {
    entry = loadCommunityDragonAssets(patch, locale);
    communityDragonCache.set(key, entry);
    entry.then(
      (assets) => readyCommunityDragon.set(key, assets),
      () => communityDragonCache.delete(key),
    );
  }
  return entry;
}

/**
 * Datos del parche de la partida a la vista. Al cambiar de idioma (`keep`), un lado que no llega
 * conserva los nombres del idioma anterior en lugar de quedarse con los identificadores de Riot.
 */
async function enrichMatch(replay: ReplayInspection, keep = false) {
  if (replay.status !== "parsed" || !replay.patch) return;
  const run = ++state.assetRun;
  const [data, community] = await Promise.allSettled([
    dataDragon(replay.dataDragonVersion ?? `${replay.patch}.1`),
    communityDragon(replay.patch),
  ]);
  if (run !== state.assetRun || state.replay?.path !== replay.path) return;
  state.dataDragon = data.status === "fulfilled" ? data.value : keep ? state.dataDragon : null;
  state.communityDragon = community.status === "fulfilled" ? community.value : keep ? state.communityDragon : null;
  renderMatchBody();
}

// ——— Imágenes de los parches ———

/** Parches de la biblioteca, del más nuevo al más viejo. */
function libraryPatches(library: ReplayLibrary | null) {
  const patches = new Set(library?.entries.map((entry) => entry.patch).filter((patch): patch is string => !!patch));
  return [...patches].sort((left, right) => compareVersions(right, left));
}

/** Datos del parche ya en memoria: DataDragon, CommunityDragon, ambos o ninguno. */
function readyPatchAssets(patch: string) {
  const locale = namesLocale();
  return {
    data: readyDataDragon.get(`${patch}.1:${locale}`) ?? null,
    community: readyCommunityDragon.get(`${patch}:${locale}`) ?? null,
  };
}

/** Espera los datos del parche, ya pedidos al leer la carpeta; si uno falla, ese lado queda vacío. */
async function loadPatchAssets(patch: string) {
  await Promise.allSettled([dataDragon(`${patch}.1`), communityDragon(patch)]);
  return readyPatchAssets(patch);
}

/**
 * Primera biblioteca con parches y sin elección guardada: antes de mostrarla se pregunta si se
 * descargan de una vez todas las imágenes de sus parches, con cuántas son y cuánto pesan. Si
 * todavía no se eligió el idioma, la misma pantalla lo pregunta arriba.
 */
async function askPrefetch(library: ReplayLibrary) {
  const run = ++askRun;
  const patches = libraryPatches(library);
  if (!patches.length) {
    cancelAsk();
    return;
  }
  state.ask = null;
  showSplash("wait");
  await Promise.race([Promise.allSettled(patches.map(loadPatchAssets)), delay(ASK_WAIT_MS)]);
  if (run !== askRun || state.splash !== "wait") return;
  let count = 0;
  let bytes = 0;
  const available = patches.filter((patch) => {
    const { data, community } = readyPatchAssets(patch);
    const images = patchImages(data, community);
    count += images.length;
    for (const image of images) bytes += AVERAGE_ICON_BYTES[image.kind];
    return images.length > 0;
  });
  // Sin datos de ningún parche (sin conexión) no hay nada que ofrecer: se pregunta otro día.
  if (!available.length) {
    leaveSplash();
    return;
  }
  showSetup({ patches: available, images: count, bytes });
}

/** Una biblioteca sin parches, o que no se pudo leer, deja sin efecto la pregunta pendiente. */
function cancelAsk() {
  askRun++;
  if (state.splash === "wait" || (state.splash === "ask" && state.ask)) leaveSplash();
}

function answerPrefetch(all: boolean) {
  const ask = state.ask;
  if (!ask || state.splash !== "ask") return;
  askRun++;
  state.ask = null;
  confirmSetupLanguage();
  void saveAssetPrefetch(all);
  if (all) startPrefetch(ask.patches, true);
  else hideSplash();
}

/** Elección desde Clientes: "Descargar todo" baja lo que falte; "Al abrir cada partida" lo detiene. */
async function chooseAssetPrefetch(all: boolean) {
  if (state.settings.assetPrefetch === all) return;
  if (!all) cancelPrefetch();
  await saveAssetPrefetch(all);
  if (all) {
    // Una elección explícita vuelve a intentar los parches que fallaron.
    attemptedPatches.clear();
    prefetchLibraryPatches();
  }
}

/** Guarda la elección. Si no se puede, vale para esta sesión y se vuelve a preguntar otro día. */
async function saveAssetPrefetch(enabled: boolean) {
  state.settings = { ...state.settings, assetPrefetch: enabled };
  renderClients();
  try {
    const saved = await invoke<AppSettings>("set_asset_prefetch", { enabled });
    // Solo esta elección: el idioma se guarda aparte y su respuesta puede llegar en otro orden.
    state.settings = { ...state.settings, assetPrefetch: saved.assetPrefetch };
  } catch {
    // Sin guardar, la elección sigue en memoria.
  }
  renderClients();
}

/** Con "Descargar todo", las imágenes de los parches que falten se bajan en segundo plano. */
function prefetchLibraryPatches() {
  if (state.settings.assetPrefetch === true) startPrefetch(libraryPatches(state.library), false);
}

function startPrefetch(patches: string[], foreground: boolean) {
  const done = prefetchedPatches();
  const pending = patches.filter((patch) => !done.has(patch) && !attemptedPatches.has(patch));
  const current = state.prefetch;
  if (current) {
    // Una descarga en marcha suma al final los parches nuevos.
    for (const patch of pending) if (!current.patches.includes(patch)) current.patches.push(patch);
    renderPrefetch();
    return;
  }
  if (!pending.length) {
    if (foreground) hideSplash();
    return;
  }
  const run: PrefetchRun = { patches: pending, index: 0, fraction: 0, foreground, cancelled: false };
  state.prefetch = run;
  if (foreground) {
    showSplash("progress");
    el.prefetchContinue.focus();
  }
  renderPrefetch();
  void runPrefetch(run);
}

/**
 * Parche por parche, cada uno en una sola llamada al backend, que descarga sus imágenes a la caché
 * en disco con su propio grupo de conexiones. Un error no detiene nada: ese parche se reintenta al
 * volver a leer la carpeta.
 */
async function runPrefetch(run: PrefetchRun) {
  for (; run.index < run.patches.length && !run.cancelled; run.index++) {
    const patch = run.patches[run.index];
    attemptedPatches.add(patch);
    run.fraction = 0;
    renderPrefetch();
    const { data, community } = await loadPatchAssets(patch);
    if (run.cancelled) break;
    // Primero las imágenes que usan las repeticiones de la biblioteca.
    const used = new Set(
      [...state.inspections.values()].filter((replay) => replay.patch === patch).flatMap(matchImageUrls),
    );
    const urls = patchImages(data, community).map((image) => image.url);
    const paths = [...urls.filter((url) => used.has(url)), ...urls.filter((url) => !used.has(url))]
      .map(cachedAssetPath)
      .filter((path): path is string => !!path);
    if (!paths.length) continue;
    const result = await prefetchPaths(run, paths);
    if (result?.cancelled) run.cancelled = true;
    if (run.cancelled) break;
    // Sin conexión (o sin respuesta del backend) no tiene sentido seguir, y no se avisa nada: los
    // parches que falten se retoman al volver a leer la carpeta.
    if (!result || result.offline) {
      run.cancelled = true;
      break;
    }
    if (result.errors === 0 && result.done === result.total && data && community) markPrefetched(patch);
  }
  if (state.prefetch === run) state.prefetch = null;
  if (run.foreground && state.splash === "progress") hideSplash();
  renderPrefetch();
  // Parches cuyos datos no estaban al preguntar, o de una biblioteca abierta mientras tanto.
  if (!run.cancelled) prefetchLibraryPatches();
}

/**
 * Una llamada a `prefetch_assets`. El backend corre una a la vez: si la de una descarga cancelada
 * todavía no volvió, primero se espera. Sin respuesta (un error del comando), `null`.
 */
async function prefetchPaths(run: PrefetchRun, paths: string[]) {
  while (prefetchCall) await prefetchCall.then(() => undefined, () => undefined);
  if (run.cancelled) return null;
  const call = invoke<AssetPrefetchResult>("prefetch_assets", { paths });
  prefetchCall = call;
  prefetchActive = run;
  try {
    return await call;
  } catch {
    return null;
  } finally {
    if (prefetchCall === call) {
      prefetchCall = null;
      prefetchActive = null;
    }
  }
}

/** Avance del parche en curso; los de una llamada cancelada que lleguen tarde no cuentan. */
function onPrefetchProgress(progress: AssetPrefetchProgress) {
  const run = prefetchActive;
  if (!run || run.cancelled || state.prefetch !== run || !(progress.total > 0)) return;
  run.fraction = Math.min(1, progress.done / progress.total);
  schedulePrefetchRender();
}

/** "Continuar": la pantalla de inicio sale y la descarga sigue en la barra superior. */
function continuePrefetchInBackground() {
  if (state.prefetch) state.prefetch.foreground = false;
  hideSplash();
}

function cancelPrefetch() {
  const run = state.prefetch;
  if (!run) return;
  run.cancelled = true;
  state.prefetch = null;
  // La llamada en curso vuelve con `cancelled: true`; la próxima espera a que vuelva.
  if (prefetchActive === run) invoke("cancel_asset_prefetch").catch(() => undefined);
  renderPrefetch();
}

function prefetchPercent(run: PrefetchRun) {
  const total = Math.max(1, run.patches.length);
  return progressPercent(((Math.min(run.index, total) + run.fraction) / total) * 100, false);
}

let prefetchFrame = 0;

/** Un avance por evento sería demasiado: se pinta una vez por cuadro. */
function schedulePrefetchRender() {
  if (prefetchFrame) return;
  prefetchFrame = requestAnimationFrame(() => {
    prefetchFrame = 0;
    renderPrefetch();
  });
}

function renderPrefetch() {
  renderSplash();
  renderAppBar();
}

function prefetchedPatches() {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(PREFETCHED_KEY) ?? "[]");
    return new Set(Array.isArray(saved) ? saved.filter((patch): patch is string => typeof patch === "string") : []);
  } catch {
    return new Set<string>();
  }
}

function markPrefetched(patch: string) {
  const done = prefetchedPatches();
  done.add(patch);
  try {
    localStorage.setItem(PREFETCHED_KEY, JSON.stringify([...done]));
  } catch {
    // Sin almacenamiento local se vuelve a revisar la próxima vez; la caché ya tiene los archivos.
  }
}

/**
 * Decodifica en segundo plano las imágenes que usan las partidas de la biblioteca (primero la
 * elegida) y los retratos de la lista. Al cambiar de partida ya están en memoria y se pintan en el
 * mismo cuadro: la partida no espera a ninguna imagen.
 */
async function warmLibraryImages() {
  const library = state.library;
  if (!library || state.splash !== "hidden") return;
  const run = ++warmRun;
  await Promise.allSettled(libraryPatches(library).map(loadPatchAssets));
  if (run !== warmRun || state.library !== library) return;
  const selected = library.entries.find((entry) => entry.path === state.selectedPath);
  const entries = selected ? [selected, ...library.entries.filter((entry) => entry !== selected)] : library.entries;
  const urls = new Set<string>();
  for (const entry of entries) {
    const replay = state.inspections.get(entry.path);
    if (replay) for (const url of matchImageUrls(replay)) urls.add(url);
  }
  for (const entry of library.entries) {
    const owner = ownerOf(entry);
    const portrait = owner ? libraryChampionIcon(owner.champion, entry.patch) : null;
    if (portrait) urls.add(portrait);
  }
  const queue = [...urls].map(cachedAssetUrl).filter((src) => !warmImages.has(src));
  const worker = async () => {
    for (let src = queue.shift(); src && run === warmRun; src = queue.shift()) await warmImage(src);
  };
  await Promise.all(Array.from({ length: WARM_WORKERS }, worker));
}

async function warmImage(src: string) {
  if (warmImages.has(src)) return;
  // Con el tope, las más viejas salen primero.
  if (warmImages.size >= WARM_LIMIT) {
    const oldest = warmImages.keys().next().value;
    if (oldest !== undefined) warmImages.delete(oldest);
  }
  const image = new Image();
  warmImages.set(src, image);
  image.src = src;
  try {
    await image.decode();
  } catch {
    // Falta o no se pudo leer: la casilla mostrará su texto.
    warmImages.delete(src);
  }
}

/** Las imágenes que muestra el marcador de una repetición, con los datos de su parche. */
function matchImageUrls(replay: ReplayInspection) {
  if (replay.status !== "parsed" || !replay.patch) return [];
  const keys = assetKeys(replay);
  const data = readyDataDragon.get(keys.data) ?? null;
  const community = readyCommunityDragon.get(keys.community) ?? null;
  const roleSlot = hasRoleItems(replay);
  const urls: Array<string | null> = [];
  for (const player of replay.players) {
    urls.push(championIconUrl(player.champion, replay.dataDragonVersion, player.championImageUrl, data));
    for (const slot of loadoutSlots(player, community)) urls.push(slot.asset?.iconUrl ?? null);
    for (const { item } of itemSlots(player, roleSlot)) if (item) urls.push(itemIconUrl(item, data));
  }
  return urls.filter((url): url is string => !!url);
}

function delay(ms: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, ms));
}

// ——— Cliente ———

function clientRequest(confirmed: ConfirmedPlan | null): ClientRequest | null {
  const replay = state.replay;
  const clientsRoot = state.settings.clientsRoot;
  if (!replay?.patch || replay.status !== "parsed" || !clientsRoot) return null;
  return {
    patch: replay.patch,
    gameVersion: replay.gameVersion,
    region: replay.region,
    locale: state.locale,
    clientsRoot,
    confirmed,
  };
}

async function openClientPlan(kind: "install" | "resume") {
  const replay = state.replay;
  if (!replay?.patch || replay.status !== "parsed" || state.install) return;
  if (!state.settings.clientsRoot && !(await chooseClientsRoot())) return;
  if (state.replay !== replay || state.install) return;
  // El plan parte del idioma que muestra la barra: el de la instalación, si ya hay una.
  state.locale = clientLocale();
  state.planKind = kind;
  state.drawerOpen = true;
  state.drawerMode = "plan";
  // Tras un fallo el plan ya se rehízo (o se está rehaciendo) para esta repetición: se usa ese.
  const request = clientRequest(null);
  const reusable = !!request && state.planKey === planKey(request) && (state.planning || !!state.plan);
  renderDrawer();
  if (!reusable) await refreshPlan();
}

/** Repetición, idioma y carpeta de un pedido de plan. */
function planKey(request: ClientRequest) {
  return [state.replay?.path ?? "", request.locale, request.clientsRoot].join("\n");
}

/** Descarta el plan mostrado y cualquier plan que siga en camino. */
function dropPlan() {
  state.planRun++;
  state.plan = null;
  state.planKey = null;
  state.planning = false;
  state.planStage = null;
  state.planError = null;
}

async function refreshPlan() {
  const request = clientRequest(null);
  if (!request) return;
  const run = ++state.planRun;
  state.plan = null;
  state.planKey = planKey(request);
  state.planning = true;
  state.planStage = null;
  state.planError = null;
  renderDrawer();
  // Una descarga detenida que el backend todavía cierra: el plan parte de lo que quede en disco.
  if (stoppedInstall) {
    await settleStoppedInstall();
    if (run !== state.planRun) return;
  }
  try {
    const plan = await invoke<ClientInstallPlan>("plan_client_install", { request });
    if (run === state.planRun) state.plan = plan;
  } catch (error) {
    if (run === state.planRun) state.planError = errorText(error);
  } finally {
    if (run === state.planRun) {
      state.planning = false;
      renderDrawer();
    }
  }
}

async function confirmInstall() {
  const plan = state.plan;
  const replay = state.replay;
  const version = replay?.gameVersion;
  if (!plan || !replay || !version || state.install || state.planning) return;
  // Si el backend todavía cierra una descarga detenida, rechazaría esta: primero se espera.
  if (stoppedInstall) {
    await settleStoppedInstall();
    if (state.plan !== plan || state.replay !== replay || state.install || state.planning) return;
  }
  const confirmed = clientRequest({
    mode: plan.mode,
    resolvedVersion: plan.resolvedVersion,
    destinationPath: plan.destinationPath,
    locale: plan.locale,
    manifestUrl: plan.manifestUrl,
  });
  if (!confirmed) return;
  // Se pide exactamente el plan que se vio, con su idioma.
  const request: ClientRequest = { ...confirmed, locale: plan.locale };
  state.installError = null;
  state.stopNote = null;
  const run: InstallRun = {
    gameVersion: version,
    build: plan.gameBuild,
    exactBuild: plan.exactBuild,
    destination: plan.destinationPath,
    locale: plan.locale,
    replayPath: replay.path,
    replayName: replay.fileName,
    replays: replaysOf(version),
    stage: "resolve",
    detail: "",
    downloaded: 0,
    total: plan.downloadBytes,
    percent: 0,
    done: false,
    stopped: false,
    samples: [],
    settled: null,
  };
  state.install = run;
  state.drawerMode = "progress";
  dropPlan();
  renderAll();
  let failed = false;
  let stopped = false;
  const pending = invoke<ClientInstallResult>("install_client", { request });
  run.settled = pending.then(
    () => undefined,
    () => undefined,
  );
  // Sin bytes nuevos el backend no avisa: un pulso por segundo repite la última muestra, así en un
  // reintento la velocidad baja y el tiempo restante no queda congelado.
  const pulse = setInterval(() => {
    if (state.install !== run || run.done || run.stage !== "download" || !run.samples.length) return;
    recordSample(run, run.downloaded);
    renderInstall();
  }, SPEED_PULSE_MS);
  try {
    const result = await pending;
    // También si se pulsó Detener justo cuando terminaba: el cliente quedó listo y así se ve.
    state.statuses.set(version, {
      gameBuild: result.gameBuild,
      exactBuild: result.exactBuild,
      destinationPath: result.destinationPath,
      state: "ready",
      executablePath: result.executablePath,
      locale: result.locale,
      installedBytes: result.totalInstallBytes,
      totalInstallBytes: result.totalInstallBytes,
    });
    state.statusVersions.add(version);
    if (state.install === run) {
      state.install = null;
      state.drawerOpen = false;
      // Solo pudo quedar el aviso de un Detener que falló durante esta misma descarga.
      state.installError = null;
    }
  } catch (error) {
    // Detenida: la interfaz ya la mostró así y lo que quedó en disco llega con la lectura de abajo.
    // Sin tocar el panel, que ya puede mostrar otro plan.
    if (run.stopped) {
      stopped = true;
    } else {
      failed = true;
      state.install = null;
      state.drawerMode = "plan";
      state.installError = { gameVersion: version, build: run.build, message: errorText(error) };
      state.planKind = "resume";
      // El panel abierto seguía esta descarga: la barra pasa a su repetición para mostrar el fallo.
      if (state.drawerOpen && state.replay?.gameVersion !== version && state.inspections.has(run.replayPath)) {
        void selectReplay(run.replayPath);
      }
    }
  } finally {
    clearInterval(pulse);
  }
  renderAll();
  await refreshClientStatuses();
  // Tras un fallo o una parada el plan se rehace solo con lo que quedó en disco, así Reanudar queda
  // listo con un clic. Si ya se pidió uno (Reanudar mientras se cerraba), se usa ese.
  if ((failed || stopped) && !state.install && !state.planning && !state.plan && state.replay?.gameVersion === version) {
    state.planKind = "resume";
    state.locale = clientLocale();
    void refreshPlan();
  }
}

/**
 * Detener se ve en el acto: sin barra de progreso ni indicador, y la compilación como incompleta.
 * El backend termina de cerrar la descarga en segundo plano (aborta las peticiones en vuelo); un
 * plan o una descarga nueva lo esperan, y al terminar se relee el disco.
 */
function stopInstall() {
  const run = state.install;
  if (!run) return;
  run.stopped = true;
  state.install = null;
  const settled: Promise<void> = (run.settled ?? Promise.resolve()).then(() => {
    if (stoppedInstall === settled) stoppedInstall = null;
  });
  stoppedInstall = settled;
  showStoppedStatus(run);
  // Lo descargado queda en disco desde la etapa de descarga: Reanudar sigue desde ahí.
  if (STAGES.indexOf(run.stage) >= STAGES.indexOf("download")) state.stopNote = { gameVersion: run.gameVersion };
  // El panel seguía la descarga: se cierra con ella.
  state.drawerOpen = false;
  state.drawerMode = "plan";
  renderAll();
  invoke("cancel_client_install").catch((error) => {
    // Sin la orden de parar, la descarga sigue: vuelve a verse, con el motivo.
    if (stoppedInstall !== settled || state.install) return;
    stoppedInstall = null;
    run.stopped = false;
    state.install = run;
    state.stopNote = null;
    dropPlan();
    state.drawerMode = "progress";
    state.installError = { gameVersion: run.gameVersion, build: run.build, message: errorText(error) };
    renderAll();
  });
}

/** Espera a que el backend termine de cerrar una descarga detenida. */
async function settleStoppedInstall() {
  while (stoppedInstall) await stoppedInstall;
}

/** Lo que la compilación tiene en disco al detener, hasta que llegue la lectura real. */
function showStoppedStatus(run: InstallRun) {
  // Una lectura empezada antes de Detener ya no puede pisar este estado.
  statusRun++;
  // `plan.json` se escribe antes de la etapa de descarga: recién desde ahí queda incompleta. Antes,
  // el disco sigue como estaba.
  if (STAGES.indexOf(run.stage) < STAGES.indexOf("download")) return;
  const previous = state.statuses.get(run.gameVersion);
  state.statuses.set(run.gameVersion, {
    gameBuild: run.build,
    exactBuild: run.exactBuild,
    destinationPath: run.destination,
    state: "partial",
    executablePath: null,
    locale: run.locale,
    installedBytes: previous?.installedBytes ?? 0,
    // Sin total no se muestra un porcentaje viejo: el real llega con la lectura del disco.
    totalInstallBytes: null,
  });
  state.statusVersions.add(run.gameVersion);
}

function showProgress() {
  if (!state.install) return;
  const entry = state.library?.entries.find((item) => item.gameVersion === state.install?.gameVersion);
  state.view = "library";
  state.drawerOpen = true;
  state.drawerMode = "progress";
  if (entry && state.replay?.gameVersion !== entry.gameVersion) void selectReplay(entry.path);
  renderAll();
}

function closeDrawer() {
  state.drawerOpen = false;
  if (!state.install) dropPlan();
  renderDrawer();
}

let progressFrame = 0;

function onProgress(progress: DownloadProgress) {
  const install = state.install;
  if (install) {
    install.stage = progress.stage;
    if (progress.stage === "download") install.detail = progress.detail;
    if (progress.stage === "download" || progress.done) {
      install.downloaded = progress.downloadedBytes;
      install.total = progress.totalBytes ?? install.total;
      install.percent = progress.percent ?? install.percent;
      install.done = install.done || progress.done;
      if (progress.stage === "download") recordSample(install, progress.downloadedBytes);
    }
  } else if (stoppedInstall) {
    // Avances de la descarga detenida mientras el backend la cierra: ya no se muestran.
    return;
  } else if (state.planning) {
    state.planStage = progress.stage;
  } else {
    return;
  }
  // Muchos eventos por segundo: se pinta una vez por cuadro.
  if (progressFrame) return;
  progressFrame = requestAnimationFrame(() => {
    progressFrame = 0;
    renderInstall();
  });
}

/**
 * Guarda los bytes de un avance y descarta los que quedan fuera de la ventana, salvo el último de
 * antes de ella, que marca su comienzo. Un avance que retrocede reinicia la medición.
 */
function recordSample(install: InstallRun, bytes: number) {
  const time = Date.now();
  const samples = install.samples;
  if (samples.length && bytes < samples[samples.length - 1].bytes) samples.length = 0;
  samples.push({ time, bytes });
  while (samples.length > 2 && samples[1].time <= time - SPEED_WINDOW_MS) samples.shift();
}

/**
 * Velocidad (bytes por segundo, promedio de los últimos ~5 s) y tiempo restante, ya formateados.
 * Hasta tener un segundo medido, vacíos. Al reanudar, `downloadedBytes` parte de lo ya guardado y
 * lo que falta es `totalBytes - downloadedBytes`.
 */
function installRate(install: InstallRun) {
  const samples = install.samples;
  const first = samples[0];
  const last = samples[samples.length - 1];
  const span = first && last ? last.time - first.time : 0;
  if (install.done || span < SPEED_MIN_SPAN_MS) return { speed: "", eta: "" };
  const speed = Math.max(0, (last.bytes - first.bytes) / (span / 1000));
  const remaining = install.total == null ? null : Math.max(0, install.total - install.downloaded);
  return {
    speed: formatSpeed(speed),
    eta: speed > 0 && remaining ? formatEta(remaining / speed) : "",
  };
}

/** Todo lo que muestra la descarga en curso: barra superior, panel, barra del cliente y Clientes. */
function renderInstall() {
  renderAppBar();
  renderDrawer();
  renderClientStrip();
  renderInstallRow();
}

async function chooseExecutable() {
  const replay = state.replay;
  if (!replay) return;
  let selected: string | string[] | null;
  try {
    selected = await open({
      multiple: false,
      filters: [{ name: "League of Legends", extensions: ["exe"] }],
    });
  } catch (error) {
    state.actionError = { path: replay.path, message: errorText(error) };
    renderClientStrip();
    return;
  }
  if (typeof selected !== "string") return;
  // Lo mismo que exige el backend al reproducir, pero dicho al elegir y no al fallar después.
  if (fileNameOf(selected).toLowerCase() === GAME_EXE.toLowerCase()) {
    state.manualExecutables.set(replay.path, selected);
    if (state.actionError?.path === replay.path) state.actionError = null;
  } else {
    state.actionError = { path: replay.path, message: t("notGameExecutable", { exe: GAME_EXE, path: selected }) };
  }
  // Se ve el resultado de la última acción: el error de un inicio anterior deja de mostrarse.
  if (state.launch?.path === replay.path && state.launch.state === "error") state.launch = null;
  renderClientStrip();
}

async function playReplay() {
  const replay = state.replay;
  const executable = clientView()?.executable;
  if (!replay || !executable || state.launch?.state === "launching") return;
  state.launch = { path: replay.path, state: "launching", message: "" };
  if (state.actionError?.path === replay.path) state.actionError = null;
  renderClientStrip();
  try {
    await invoke("launch_replay", {
      request: { replayPath: replay.path, executablePath: executable, locale: clientLocale() },
    });
    if (state.launch?.path === replay.path) state.launch = null;
    countPlaying(replay.path, 1);
  } catch (error) {
    // Se relee antes de guardar el error para que un cambio de Vanguard no borre este fallo.
    await refreshVanguard();
    state.launch = { path: replay.path, state: "error", message: errorText(error) };
  } finally {
    renderClientStrip();
  }
}

/** El juego que abrió una repetición se cerró: la barra vuelve a su estado normal. */
function onReplayExited({ replayPath }: ReplayExited) {
  countPlaying(replayPath, -1);
  renderClientStrip();
}

// Un cierre puede llegar antes que la respuesta del inicio: la cuenta admite valores negativos.
function countPlaying(path: string, delta: number) {
  const key = pathKey(path);
  const count = (state.playing.get(key) ?? 0) + delta;
  if (count === 0) state.playing.delete(key);
  else state.playing.set(key, count);
}

function isPlaying(path: string) {
  return (state.playing.get(pathKey(path)) ?? 0) > 0;
}

/** Rutas de Windows: sin distinguir mayúsculas ni el tipo de barra. */
function pathKey(path: string) {
  return path.replace(/\//g, "\\").toLowerCase();
}

/** Repeticiones de la biblioteca con esa versión; al menos la que se está viendo. */
function replaysOf(version: string) {
  return Math.max(1, state.library?.entries.filter((entry) => entry.gameVersion === version).length ?? 0);
}

// Solo lectura: ChronoRift nunca detiene Vanguard ni cambia su configuración.
async function refreshVanguard() {
  let next: VanguardState = "unknown";
  try {
    next = await invoke<VanguardState>("vanguard_status");
  } catch {
    // Sin respuesta del backend el estado queda desconocido y no se muestra.
  }
  if (next === state.vanguard) return;
  // Un inicio fallido con Vanguard activo deja de ser el estado actual cuando se detiene.
  if (state.vanguard === "running" && state.launch?.state === "error") state.launch = null;
  state.vanguard = next;
  renderClientStrip();
}

function clientView(): ClientView | null {
  const replay = state.replay;
  if (!replay) return null;
  const version = replay.gameVersion;
  const status = version ? state.statuses.get(version) : undefined;
  const manual = state.manualExecutables.get(replay.path) ?? null;
  const install = state.install;
  // Mientras se descarga otra compilación no se puede empezar esta.
  const plan = (kind: "install" | "resume") => ({ run: () => void openClientPlan(kind), disabled: !!install });
  const none: Omit<ClientView, "glyph" | "text" | "location"> = {
    speed: "",
    build: version ?? "—",
    inexact: false,
    locationMuted: false,
    reveal: null,
    action: null,
    executable: null,
    progress: null,
    locale: clientLocale(),
    error: null,
    note: null,
    tone: null,
  };
  let view: ClientView;

  if (install && install.gameVersion === version) {
    view = {
      ...none,
      glyph: "busy",
      text: installStateText(install),
      speed: installRate(install).speed,
      build: install.build,
      inexact: !install.exactBuild,
      location: install.destination,
      reveal: install.destination,
      action: { label: t("progress"), icon: "download", run: showProgress },
      progress: downloadPercent(install.percent, install.done),
    };
  } else if (manual) {
    view = { ...none, glyph: "ready", text: t("chosenExecutable"), location: manual, reveal: manual, executable: manual };
  } else if (replay.status !== "parsed" || !replay.patch) {
    view = { ...none, glyph: "missing", text: t("noBuild"), location: "—", locationMuted: true, locale: "—" };
  } else if (!state.settings.clientsRoot) {
    view = {
      ...none,
      glyph: "missing",
      text: t("noClientsFolder"),
      location: "—",
      locationMuted: true,
      action: { label: t("chooseFolder"), icon: "folder", run: () => void chooseClientsRoot() },
    };
  } else if (state.statusesError) {
    // Carpeta de clientes inaccesible: no se sabe si hay cliente, así que no se ofrece nada.
    view = {
      ...none,
      glyph: "unknown",
      text: t("unavailable"),
      location: "—",
      locationMuted: true,
      error: state.statusesError,
      tone: "warn",
    };
  } else if (!version || !state.statusVersions.has(version)) {
    // Estado todavía sin leer: neutro, ni "Listo" ni "No instalado".
    view = { ...none, glyph: "unknown", text: "—", location: "—", locationMuted: true };
  } else if (status?.state === "ready" && status.executablePath) {
    const executable = status.executablePath;
    view = {
      ...none,
      glyph: "ready",
      text: t("ready"),
      build: status.gameBuild,
      inexact: status.exactBuild === false,
      location: status.destinationPath,
      reveal: executable,
      executable,
    };
  } else if (status?.state === "partial") {
    view = {
      ...none,
      glyph: "partial",
      text: partialStateText(status),
      build: status.gameBuild,
      inexact: status.exactBuild === false,
      location: status.destinationPath,
      reveal: status.destinationPath,
      action: { label: t("resume"), icon: "download", ...plan("resume") },
      // Tras Detener: lo descargado sigue en disco y Reanudar continúa desde ahí.
      note: state.stopNote?.gameVersion === version ? t("stoppedNote") : null,
    };
  } else {
    view = {
      ...none,
      glyph: "missing",
      text: t("notInstalled"),
      location: status?.destinationPath ?? "—",
      locationMuted: true,
      action: { label: t("prepareClient"), icon: "download", ...plan("install") },
    };
  }

  const launch = state.launch?.path === replay.path ? state.launch : null;
  let launchFailed = false;
  if (launch?.state === "launching") view.text = t("opening");
  else if (view.executable && state.vanguard === "running") {
    // Con Vanguard activo el juego no abre: eso manda sobre "Reproduciendo".
    view.glyph = "blocked";
    view.text = t("vanguardActive");
  } else if (view.executable && isPlaying(replay.path)) view.text = t("playing");
  else if (launch?.state === "error") {
    view.text = t("playFailed");
    launchFailed = true;
  }
  // El tono sigue a la marca; un inicio fallido queda neutro y su motivo va en rojo debajo.
  view.tone = launchFailed ? null : view.tone ?? GLYPH_TONES[view.glyph] ?? null;

  // Una sola línea: el error de la última acción sobre esta partida y, si no hay, el de la carpeta
  // de clientes o el de la última descarga. Esa descarga pudo ser de otra compilación (se cambió de
  // partida mientras corría): igual se ve, con su build delante, para que no falle en silencio.
  const failure = manual ? null : state.installError;
  const download = failure
    ? failure.gameVersion === version ? failure.message : `${failure.build} · ${failure.message}`
    : null;
  view.error = (launch?.state === "error" ? launch.message : null)
    ?? (state.actionError?.path === replay.path ? state.actionError.message : null)
    ?? view.error
    ?? download;
  // Una sola línea bajo la barra: un error manda sobre la nota.
  if (view.error) view.note = null;
  return view;
}

/** "Incompleto · 20,4 %": lo que hay en disco (con lo guardado a medias) sobre el total. */
function partialStateText(status: LocalClientStatus) {
  const percent = status.totalInstallBytes
    ? downloadPercent((status.installedBytes / status.totalInstallBytes) * 100, false)
    : null;
  return percent == null ? t("incomplete") : t("incompletePercent", { percent: formatDownloadPercent(percent) });
}

/**
 * Idioma con el que se muestra y se lanza el cliente. Un cliente descargado por ChronoRift solo
 * tiene los archivos del idioma de su instalación, así que ese manda; con un ejecutable elegido
 * o sin cliente se usa el idioma sugerido por la repetición o elegido en el plan.
 */
function clientLocale() {
  const replay = state.replay;
  if (!replay) return state.locale;
  const install = state.install;
  if (install && install.gameVersion === replay.gameVersion) return install.locale;
  if (state.manualExecutables.has(replay.path)) return state.locale;
  const status = replay.gameVersion ? state.statuses.get(replay.gameVersion) : undefined;
  return (status?.state === "ready" || status?.state === "partial") && status.locale
    ? status.locale
    : state.locale;
}

// ——— Render ———

function renderAll() {
  renderAppBar();
  renderLibrary();
  renderMatch();
  renderDrawer();
  renderClients();
}

function renderAppBar() {
  for (const item of el.navItems) {
    if (item.dataset.view === state.view) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
  el.libraryView.hidden = state.view !== "library";
  el.clientsView.hidden = state.view !== "clients";
  el.skipLink.setAttribute("href", `#${(state.view === "clients" ? el.clientsMain : el.matchMain).id}`);
  // La descarga del cliente manda; si no hay, las imágenes que se bajan en segundo plano.
  const install = state.install;
  const images = !install && state.prefetch && !state.prefetch.foreground ? state.prefetch : null;
  el.activity.hidden = !install && !images;
  // La velocidad solo es de la descarga del cliente; su ancho queda reservado desde el comienzo.
  el.activitySpeed.hidden = !install;
  if (install) {
    const percent = downloadPercent(install.percent, install.done);
    setText(el.activityLabel, t("downloading"));
    setText(el.activityBuild, install.build);
    el.activityBar.style.width = `${percent}%`;
    setText(el.activityPercent, formatDownloadPercent(percent));
    setText(el.activitySpeed, installRate(install).speed || "—");
    // El tiempo restante no cabe en la barra: queda en el panel y aquí, al pasar el puntero.
    el.activity.title = rateText(install);
  } else if (images) {
    el.activity.title = "";
    const percent = prefetchPercent(images);
    setText(el.activityLabel, t("images"));
    setText(el.activityBuild, images.patches[Math.min(images.index, images.patches.length - 1)] ?? "—");
    el.activityBar.style.width = `${percent}%`;
    setText(el.activityPercent, formatPercent(percent));
  }
  el.storageRoot.textContent = state.settings.clientsRoot ?? t("noFolder");
  el.storageChip.title = state.settings.clientsRoot ?? "";
}

function renderLibrary() {
  const library = state.library;
  const entries = visibleEntries();
  const failed = visibleFailures();
  // Cuenta cada archivo de la carpeta, también los que no se pudieron leer.
  const total = (library?.entries.length ?? 0) + (library?.failures.length ?? 0);
  const shown = entries.length + failed.length;
  const filtered = state.query.trim() !== "" || state.filter !== "all";
  // Sin carpeta abierta no hay cuenta; la ruta se ve también si falló, para reintentar.
  el.replayCount.hidden = !library;
  el.replayCount.textContent = filtered ? `${shown}/${total}` : String(total);
  el.refreshLibrary.disabled = !state.libraryPath || state.libraryBusy;
  el.openLibrary.disabled = state.libraryBusy;
  el.search.disabled = total === 0 || state.libraryBusy;
  el.folderPath.hidden = !state.libraryPath;
  el.folderPath.textContent = state.libraryPath ? shortenPath(state.libraryPath) : "";
  el.folderPath.title = state.libraryPath ?? "";

  const owned = library?.entries.map(ownerOf).filter((owner): owner is LibraryPlayer => !!owner) ?? [];
  el.resultFilter.hidden = !state.owner;
  const counts: Record<Filter, number> = {
    all: total,
    win: owned.filter((owner) => owner.win).length,
    loss: owned.filter((owner) => !owner.win).length,
  };
  for (const button of el.resultFilter.querySelectorAll<HTMLButtonElement>("[data-filter]")) {
    const filter = button.dataset.filter as Filter;
    button.setAttribute("aria-pressed", String(filter === state.filter));
    const count = button.querySelector("[data-count]");
    if (count) count.textContent = String(counts[filter]);
  }

  renderRows(entries, failed);
  // La lista vacía se oculta para que el estado vacío ocupe todo el alto y quede centrado.
  el.libraryList.hidden = shown === 0;
  el.libraryEmpty.hidden = shown > 0;
  const openButton = el.libraryEmpty.querySelector<HTMLButtonElement>("button");
  if (openButton) openButton.hidden = state.libraryBusy || (!!library && !state.libraryError);
  el.libraryEmptyText.textContent = state.libraryBusy
    ? t("loadingEllipsis")
    : state.libraryError
      ? t("folderLoadFailed", { error: state.libraryError })
      : !library
        ? t("noFolder")
        : total === 0
          ? t("noReplays")
          : t("noResults");

  el.libraryOwner.hidden = !state.owner;
  el.libraryOwnerName.textContent = state.owner ?? "";
  el.libraryOwnerName.title = state.owner ?? "";
  const versions = new Set(library?.entries.map((entry) => entry.gameVersion).filter(Boolean) as string[]);
  const known = [...versions].every((version) => state.statusVersions.has(version));
  const ready = [...versions].filter((version) => state.statuses.get(version)?.state === "ready").length;
  const failures = library?.failures.length ? t("failedFiles", { count: library.failures.length }) : "";
  // La cuenta de builds listos espera a que se lean los estados.
  const builds = library && state.settings.clientsRoot && versions.size && known
    ? t("buildsReady", { ready, total: versions.size })
    : "";
  el.librarySummary.textContent = [builds, failures].filter(Boolean).join(" · ");
}

function renderRows(entries: ReplayLibraryEntry[], failures: ReplayLibrary["failures"]) {
  const library = state.library;
  if (rowsLibrary !== library) {
    // Otra carpeta u otro escaneo: se olvidan las filas de archivos que ya no están.
    const paths = new Set(library?.entries.map((entry) => entry.path));
    for (const path of replayRows.keys()) if (!paths.has(path)) replayRows.delete(path);
    failureRows.clear();
    rowsLibrary = library;
  }
  const active = document.activeElement;
  const focused = active instanceof HTMLElement && active.parentElement === el.libraryList ? active.dataset.path : undefined;
  reconcileChildren(el.libraryList, [...entries.map(replayRow), ...failures.map(failureRow)]);
  // Si la fila con el foco tuvo que moverse, el foco vuelve a ella.
  const row = focused ? replayRows.get(focused)?.element : undefined;
  if (row?.isConnected && document.activeElement !== row) row.focus({ preventScroll: true });
}

/** Deja en `parent` exactamente `nodes`, en orden, moviendo solo lo que no está en su sitio. */
function reconcileChildren(parent: HTMLElement, nodes: Node[]) {
  let cursor = parent.firstChild;
  for (const node of nodes) {
    if (node === cursor) {
      cursor = cursor.nextSibling;
      continue;
    }
    parent.insertBefore(node, cursor);
  }
  while (cursor) {
    const next = cursor.nextSibling;
    parent.removeChild(cursor);
    cursor = next;
  }
}

/** Archivo que no se pudo leer: fila gris con el nombre y el motivo, sin selección. */
function failureRow(failure: ReplayLibrary["failures"][number]) {
  const key = `${failure.path}\n${failure.message}`;
  let row = failureRows.get(key);
  if (!row) {
    row = h(
      "div",
      { class: "failed-row", title: failure.path },
      h("span"),
      h("span", { class: "tile", text: "—" }),
      h(
        "span",
        { class: "replay-row__main" },
        h("span", { class: "replay-row__name", text: fileNameOf(failure.path) }),
        h("span", { class: "failed-row__reason", text: failure.message, title: failure.message }),
      ),
    );
    failureRows.set(key, row);
  }
  return row;
}

/**
 * Fila de una repetición. El botón se conserva entre renders (el foco no se pierde al elegir con
 * Enter); su contenido solo se rehace si cambió, y la selección y el estado se actualizan en el sitio.
 */
function replayRow(entry: ReplayLibraryEntry) {
  let record = replayRows.get(entry.path);
  if (!record) {
    const element = h("button", {
      class: "replay-row",
      title: entry.path,
      dataset: { path: entry.path },
      attrs: { type: "button" },
      on: { click: () => void selectReplay(entry.path) },
    });
    record = { element, glyph: glyphElement("unknown"), entry: null, content: "" };
    replayRows.set(entry.path, record);
  }
  const owner = ownerOf(entry);
  const parsed = entry.status === "parsed";
  const name = owner ? libraryChampionName(owner.champion) : entry.matchId ?? entry.fileName;
  // Sin jugadores (no analizada, o estadísticas omitidas) no se inventa un 0 – 0.
  const scored = parsed && entry.players.length > 0;
  const kda = owner ? `${owner.kills}/${owner.deaths}/${owner.assists}` : scored ? `${entry.blueKills} – ${entry.redKills}` : "—";
  const portrait = owner ? libraryChampionIcon(owner.champion, entry.patch) : null;
  // Con el idioma: la fecha, "Perfecto" y la letra del resultado cambian con él.
  const content = [owner?.name ?? "", name, kda, portrait ?? "", currentLanguage()].join("\n");
  if (record.entry !== entry || record.content !== content) {
    record.entry = entry;
    record.content = content;
    record.element.replaceChildren(...replayRowContent(entry, owner, name, kda, portrait, record.glyph));
  }
  const glyph = glyphFor(entry.gameVersion);
  record.glyph.dataset.state = glyph;
  record.element.classList.toggle("is-invalid", !parsed);
  setAttribute(record.element, "aria-current", String(entry.path === state.selectedPath));
  setAttribute(
    record.element,
    "aria-label",
    [
      owner ? t(owner.win ? "victory" : "defeat") : null,
      name,
      parsed ? (scored ? kda : null) : t("invalid"),
      entry.gameDurationMs == null ? null : formatDuration(entry.gameDurationMs),
      entry.patch ? t("patchNamed", { patch: entry.patch }) : null,
      entry.gameVersion ? glyphLabel(glyph) : null,
    ].filter(Boolean).join(", "),
  );
  return record.element;
}

function replayRowContent(
  entry: ReplayLibraryEntry,
  owner: LibraryPlayer | null,
  name: string,
  kda: string,
  portrait: string | null,
  glyph: HTMLElement,
) {
  return [
    h(
      "span",
      {},
      owner
        ? h("span", { class: `result-badge ${owner.win ? "is-win" : "is-loss"}`, text: t(owner.win ? "winLetter" : "lossLetter") })
        : null,
    ),
    owner
      ? h("span", { class: "tile", title: name }, assetImage(portrait, name, abbreviate(name), true))
      : h("span", { class: "tile", text: entry.region?.slice(0, 3) ?? "—" }),
    h(
      "span",
      { class: "replay-row__main" },
      h(
        "span",
        { class: "replay-row__title" },
        h("span", { class: "replay-row__name", text: name }),
        owner?.position ? h("span", { class: "replay-row__pos", text: POSITIONS[owner.position] ?? owner.position }) : null,
      ),
      h(
        "span",
        { class: "replay-row__sub" },
        h("span", { text: kda }),
        owner ? h("span", { text: kdaRatio(owner.kills, owner.deaths, owner.assists, 1) }) : null,
      ),
    ),
    h(
      "span",
      { class: "replay-row__side" },
      h("span", { class: "replay-row__date", text: formatDate(entry.modifiedAtMs) }),
      h(
        "span",
        { class: "replay-row__meta" },
        h("span", { text: formatDuration(entry.gameDurationMs) }),
        h("span", { class: "replay-row__patch" }, entry.patch ?? "—", glyph),
      ),
    ),
  ];
}

function glyphFor(version: string | null): GlyphState {
  if (!version) return "missing";
  if (state.install?.gameVersion === version) return "busy";
  if (!state.settings.clientsRoot) return "missing";
  // Sin lectura del disco todavía (o con la carpeta inaccesible) el estado es neutro.
  if (!state.statusVersions.has(version)) return "unknown";
  const status = state.statuses.get(version)?.state;
  return status === "ready" || status === "partial" ? status : "missing";
}

function glyphLabel(glyph: GlyphState) {
  const key = ({
    ready: "glyphReady",
    partial: "glyphPartial",
    missing: "glyphMissing",
    busy: "glyphBusy",
    blocked: "glyphBlocked",
    unknown: null,
  } as const)[glyph];
  return key ? t(key) : null;
}

function glyphElement(glyph: GlyphState) {
  return h("span", { class: "glyph", dataset: { state: glyph } });
}

function renderMatch() {
  const replay = state.replay;
  el.workspace.hidden = !replay;
  el.replayEmpty.hidden = !!replay;
  // Mientras se lee otra repetición la actual sigue a la vista; el CSS la atenúa solo si tarda.
  el.workspace.setAttribute("aria-busy", String(state.replayLoading));
  if (!replay) {
    el.emptyTitle.textContent = state.replayLoading
      ? t("loadingEllipsis")
      : state.replayError
        ? t("replayOpenFailed")
        : t("noReplaySelected");
    el.emptyDetail.textContent = state.replayLoading ? "" : state.replayError ?? "";
    return;
  }

  const entry = state.library?.entries.find((item) => item.path === replay.path);
  el.fileName.textContent = replay.fileName;
  el.fileName.title = replay.path;
  el.fileSize.textContent = entry ? formatBytes(entry.fileSize) : "—";
  el.matchDate.textContent = formatDateLong(entry?.modifiedAtMs ?? null);

  const owner = replay.players.find((player) => player.name === state.owner) ?? null;
  const blueWon = replay.winningTeam === "Blue";
  const redWon = replay.winningTeam === "Red";
  if (owner) {
    setChip(el.resultChip, t(owner.win ? "victory" : "defeat"), owner.win ? "win" : "loss");
    el.resultSide.textContent = owner.team === "Blue" || owner.team === "Red" ? sideLabel(owner.team) : "";
    setTeam(el.resultSide, owner.team);
  } else {
    setChip(el.resultChip, t(blueWon ? "blueWins" : redWon ? "redWins" : "noResult"), blueWon || redWon ? "solid" : "outline");
    el.resultSide.textContent = "";
    setTeam(el.resultSide, null);
  }
  // Sin jugadores (no analizada, o estadísticas omitidas) no hay marcador: un solo "—" en lugar de
  // un 0 – 0 inventado.
  const scored = replay.status === "parsed" && replay.players.length > 0;
  el.score.classList.toggle("score--empty", !scored);
  el.blueScore.textContent = scored ? String(replay.blueKills) : "—";
  el.redScore.textContent = scored ? String(replay.redKills) : "—";
  el.blueScore.classList.toggle("is-winner", blueWon);
  el.redScore.classList.toggle("is-winner", redWon);
  el.score.setAttribute(
    "aria-label",
    scored
      ? t("scoreLabel", { result: el.resultChip.textContent ?? "", blue: replay.blueKills, red: replay.redKills })
      : el.resultChip.textContent ?? "",
  );
  el.duration.textContent = formatDuration(replay.gameDurationMs);
  el.patch.textContent = replay.patch ?? "—";
  el.region.textContent = replay.region ?? "—";
  const matchPage = matchPageUrl(replay);
  el.matchId.replaceChildren(replay.matchId ?? "—", matchPage ? icon("external") : "");
  el.matchId.disabled = !matchPage;
  el.matchId.title = matchPage ? t("openInLeagueOfGraphs") : "";
  // Sin ID no hay nada que copiar.
  el.copyMatchId.hidden = !replay.matchId;

  for (const tab of el.tabs) tab.setAttribute("aria-pressed", String(tab.dataset.tab === state.tab));
  el.panelBoard.hidden = state.tab !== "board";
  el.panelStats.hidden = state.tab !== "stats";
  el.panelFile.hidden = state.tab !== "file";
  renderClientStrip();
  renderMatchBody();
}

function renderMatchBody() {
  const replay = state.replay;
  if (!replay) return;
  if (state.tab === "board") renderScoreboard(replay);
  if (state.tab === "stats") renderStats(replay);
  if (state.tab === "file") renderFile(replay);
}

// Solo se escribe lo que cambió: el estado y el error son regiones vivas y se pintan en cada cuadro
// de una descarga; reescribir el mismo texto los volvería a anunciar.
function renderClientStrip() {
  const view = clientView();
  if (!view) return;
  el.clientGlyph.dataset.state = view.glyph;
  setTone(el.clientStatus, view.tone);
  setText(el.clientText, view.text);
  setText(el.version, view.build);
  el.version.title = view.inexact && state.replay?.gameVersion ? t("replayVersion", { version: state.replay.gameVersion }) : "";
  el.versionInexact.hidden = !view.inexact;
  setText(el.clientLocation, view.location);
  el.clientLocation.title = view.location === "—" ? "" : view.location;
  el.clientLocation.classList.toggle("is-muted", view.locationMuted);
  el.revealClient.hidden = !view.reveal;
  setText(el.clientLocale, view.locale);
  el.clientAction.hidden = !view.action;
  if (view.action) {
    setText(el.clientActionLabel, view.action.label);
    if (el.clientActionIcon.dataset.iconName !== view.action.icon) {
      el.clientActionIcon.replaceChildren(icon(view.action.icon, "icon icon--sm"));
      el.clientActionIcon.dataset.iconName = view.action.icon;
    }
    el.clientAction.disabled = !!view.action.disabled;
  }
  el.play.disabled = !view.executable || state.launch?.state === "launching";
  el.clientProgress.hidden = view.progress == null;
  el.clientProgressBar.style.width = `${view.progress ?? 0}%`;
  setText(el.clientSpeed, view.speed);
  el.clientError.hidden = !view.error;
  setText(el.clientError, view.error ?? "");
  el.clientError.title = view.error ?? "";
  el.clientNote.hidden = !view.note;
  setText(el.clientNote, view.note ?? "");
}

// ——— Marcador ———

/** Sin jugadores: una repetición que no se pudo analizar dice por qué (notas del backend). */
function emptyMatchText(replay: ReplayInspection) {
  // Las notas explican una lectura fallida o estadísticas omitidas (más de 32 jugadores).
  return replay.notes.length ? replay.notes.join(" ") : t("noStats");
}

function renderScoreboard(replay: ReplayInspection) {
  const empty = replay.players.length === 0;
  el.boardHead.hidden = empty;
  if (empty) {
    el.teams.replaceChildren(h("p", { class: "muted", text: emptyMatchText(replay) }));
    return;
  }
  const maxDamage = Math.max(1, ...replay.players.map((player) => player.damageToChampions));
  const minutes = (replay.gameDurationMs ?? 0) / 60_000;
  el.teams.replaceChildren(
    ...teamSides(replay).map((side) => {
      const players = replay.players.filter((player) => player.team === side);
      const summary = replay.teams.find((team) => team.team === side);
      const kills = summary?.kills ?? players.reduce((total, player) => total + player.kills, 0);
      const win = summary?.win ?? players.some((player) => player.win);
      const label = sideLabel(side);
      return h(
        "section",
        { class: "sb-team", attrs: { "aria-label": t("teamResult", { side: label, result: t(win ? "victoryLower" : "defeatLower") }) } },
        h(
          "div",
          { class: "sb-team__head" },
          h("h3", { class: "side-label", text: label, dataset: teamData(side) }),
          chip(t(win ? "victory" : "defeat"), win ? "win" : "loss", true),
          summary ? objectives(summary) : null,
        ),
        ...players.map((player) => playerRow(player, kills, maxDamage, minutes, replay)),
      );
    }),
  );
}

function objectives(team: TeamSummary) {
  // Los asesinatos repiten el marcador de arriba: con poco ancho el CSS los quita primero.
  const pairs: Array<[string, string, string?]> = [
    [t("kills"), String(team.kills), "objectives__kills"],
    [t("towers"), String(team.towers)],
    [t("inhibShort"), String(team.inhibitors)],
    [t("dragons"), String(team.dragons)],
    [t("barons"), String(team.barons)],
    [t("heralds"), String(team.heralds)],
    [t("grubs"), String(team.voidgrubs)],
  ];
  if (team.atakhan) pairs.push(["Atakhan", String(team.atakhan)]);
  pairs.push([t("gold"), formatThousands(team.goldEarned)]);
  return h(
    "dl",
    { class: "objectives" },
    ...pairs.map(([label, value, extra]) => h("div", { class: extra }, h("dt", { text: label }), h("dd", { text: value }))),
  );
}

function playerRow(player: PlayerStats, teamKills: number, maxDamage: number, minutes: number, replay: ReplayInspection) {
  const owner = player.name === state.owner;
  const champion = championName(player.champion);
  const kda = `${player.kills} / ${player.deaths} / ${player.assists}`;
  return h(
    "div",
    {
      class: `sb-row sb-player${owner ? " is-owner" : ""}`,
      // Un div genérico no admite nombre accesible: como grupo, el resumen se anuncia al entrar.
      attrs: {
        role: "group",
        "aria-label": `${player.name || t("player")}, ${champion}, ${player.kills}/${player.deaths}/${player.assists}`,
      },
    },
    h(
      "div",
      { class: "sb-champ" },
      championTile(player.champion, replay.dataDragonVersion, player.championImageUrl),
      player.level ? h("span", { class: "level", title: t("level"), text: player.level }) : null,
    ),
    h(
      "div",
      { class: "sb-name" },
      h(
        "div",
        { class: "sb-name__line" },
        h("span", { class: "sb-name__id", text: player.gameName || player.name || "—", title: player.name }),
        player.tagLine ? h("span", { class: "sb-name__tag", text: `#${player.tagLine}` }) : null,
        owner ? h("span", { class: "owner-tag", text: t("you") }) : null,
      ),
      h("span", { class: "sb-name__sub", text: [POSITIONS[player.position], champion].filter(Boolean).join(" · ") }),
    ),
    h("div", { class: "loadout sb-col--loadout" }, ...loadout(player)),
    h(
      "div",
      { class: "kda" },
      h("span", { class: "kda__line", text: kda }),
      h("span", { class: "kda__ratio", text: kdaRatio(player.kills, player.deaths, player.assists) }),
    ),
    h("span", {
      class: "sb-num sb-num--muted sb-col--kp",
      text: teamKills ? formatPercent(((player.kills + player.assists) / teamKills) * 100) : "—",
    }),
    h(
      "div",
      { class: "bar" },
      h("div", { class: "bar__track" }, h("div", { class: "bar__fill", style: { width: `${(player.damageToChampions / maxDamage) * 100}%` } })),
      h("span", { class: "bar__value", text: formatInt(player.damageToChampions) }),
    ),
    h("span", { class: "sb-num", text: formatThousands(player.goldEarned) }),
    h(
      "div",
      { class: "sb-cs" },
      h("span", { class: "sb-num", text: player.creepScore }),
      minutes > 0 ? h("small", { text: `${formatDecimal(player.creepScore / minutes, 1)}/min` }) : null,
    ),
    h("span", { class: "sb-num sb-col--vision", text: player.visionScore }),
    h("div", { class: "items" }, ...items(player, hasRoleItems(replay))),
  );
}

/** Hechizos, piedra angular y rama secundaria: lo mismo para el marcador y para precargar. */
function loadoutSlots(player: PlayerStats, community = state.communityDragon) {
  const slots: Array<[string | null, Map<string, MatchAsset> | undefined, boolean]> = [
    [player.summonerSpells[0] ?? null, community?.spells, false],
    [player.summonerSpells[1] ?? null, community?.spells, false],
    [player.keystone ?? player.perks[0] ?? null, community?.perks, true],
    [player.subStyle, community?.styles, true],
  ];
  return slots.map(([id, assets, round]) => ({ id, round, asset: id ? assets?.get(id) : undefined }));
}

function loadout(player: PlayerStats) {
  return loadoutSlots(player).map(({ id, round, asset }) => {
    const label = asset?.name ?? id ?? "";
    return h(
      "span",
      { class: `slot${round ? " slot--round" : ""}${id ? "" : " slot--empty"}`, title: label },
      id ? assetImage(asset?.iconUrl ?? null, label, abbreviate(label)) : null,
    );
  });
}

/** Desde las misiones de rol de 2026 hay una ranura más (7); las partidas anteriores no la tienen. */
function hasRoleItems(replay: ReplayInspection) {
  return replay.players.some((player) => player.items.some((item) => item.slot === 7));
}

/** Inventario (0–5), objeto de la misión de rol (7) si la partida lo usa y abalorio (6). */
function itemSlots(player: PlayerStats, roleSlot: boolean) {
  const slots = [0, 1, 2, 3, 4, 5, ...(roleSlot ? [7] : []), 6];
  return slots.map((slot) => ({ slot, item: player.items.find((candidate) => candidate.slot === slot) }));
}

function itemIconUrl(item: PlayerItem, data = state.dataDragon) {
  return data?.items.get(item.id)?.iconUrl ?? item.imageUrl;
}

function items(player: PlayerStats, roleSlot: boolean) {
  return itemSlots(player, roleSlot).map(({ slot, item }) => {
    const asset = item ? state.dataDragon?.items.get(item.id) : undefined;
    const label = item ? asset?.name ?? t("itemNamed", { id: item.id }) : "";
    const classes = ["slot", "slot--item"];
    if (slot === 7) classes.push("slot--role");
    if (!item) classes.push("slot--empty");
    if (slot === 6) classes.push("slot--trinket");
    return h(
      "span",
      { class: classes.join(" "), title: label },
      item ? assetImage(itemIconUrl(item), label, abbreviate(asset?.name ?? item.id)) : null,
    );
  });
}

// ——— Estadísticas ———

function renderStats(replay: ReplayInspection) {
  const empty = replay.players.length === 0;
  el.statsEmpty.hidden = !empty;
  el.statsEmpty.textContent = emptyMatchText(replay);
  el.statsTop.hidden = empty;
  el.statsPlayers.hidden = empty;
  if (empty) return;
  renderTeamCompare(replay);
  renderDamage(replay);
  renderPlayerTable(replay);
}

function renderTeamCompare(replay: ReplayInspection) {
  const blue = replay.teams.find((team) => team.team === "Blue");
  const red = replay.teams.find((team) => team.team === "Red");
  if (!blue || !red) {
    el.teamCompare.replaceChildren();
    return;
  }
  const rows: Array<[string, number, number, (value: number) => string]> = [
    [t("kills"), blue.kills, red.kills, String],
    [t("gold"), blue.goldEarned, red.goldEarned, formatThousands],
    [t("damageToChampions"), blue.damageToChampions, red.damageToChampions, formatThousands],
    [t("towers"), blue.towers, red.towers, String],
    [t("inhibitors"), blue.inhibitors, red.inhibitors, String],
    [t("dragons"), blue.dragons, red.dragons, String],
    [t("barons"), blue.barons, red.barons, String],
    [t("heralds"), blue.heralds, red.heralds, String],
    [t("voidgrubs"), blue.voidgrubs, red.voidgrubs, String],
    ["Atakhan", blue.atakhan, red.atakhan, String],
  ];
  el.teamCompare.replaceChildren(
    ...rows
      .filter(([, left, right]) => left + right > 0)
      .map(([label, left, right, format]) => {
        const lead = left === right ? "tie" : left > right ? "left" : "right";
        const tone = (side: "left" | "right") => (lead === "tie" ? "is-tie" : lead === side ? "is-lead" : "is-trail");
        return h(
          "div",
          { class: "compare-row", attrs: { role: "group", "aria-label": t("compareLabel", { label, blue: format(left), red: format(right) }) } },
          h("span", { class: "compare-row__label", text: label }),
          h("span", { class: `compare-row__value num${lead === "left" ? " is-lead" : ""}`, text: format(left) }),
          h(
            "div",
            { class: "split", attrs: { "aria-hidden": "true" } },
            h("span", { class: tone("left"), style: { "flex-grow": String(left) } }),
            h("span", { class: tone("right"), style: { "flex-grow": String(right) } }),
          ),
          h("span", { class: `compare-row__value${lead === "right" ? " is-lead" : ""}`, text: format(right) }),
        );
      }),
  );
}

function renderDamage(replay: ReplayInspection) {
  const maxDamage = Math.max(1, ...replay.players.map((player) => player.damageToChampions));
  el.damageChart.replaceChildren(
    ...teamSides(replay).flatMap((side) => [
      h("div", { class: "group-label side-label", text: sideLabel(side).toUpperCase(), dataset: teamData(side) }),
      ...replay.players
        .filter((player) => player.team === side)
        .map((player) => {
          const owner = player.name === state.owner;
          const segments: Array<[string, number, string]> = [
            ["physical", player.physicalDamageToChampions, t("physical")],
            ["magic", player.magicDamageToChampions, t("magic")],
            ["true", player.trueDamageToChampions, t("trueDamage")],
          ];
          const champion = championName(player.champion);
          return h(
            "div",
            {
              class: `dmg-row${owner ? " is-owner" : ""}`,
              attrs: {
                role: "group",
                "aria-label": `${champion}: ${formatInt(player.damageToChampions)}; ${segments
                  .map(([, value, label]) => `${label.toLowerCase()} ${formatInt(value)}`)
                  .join(", ")}`,
              },
            },
            h("span", { class: "dmg-row__name", text: champion, title: champion }),
            h(
              "div",
              { class: "dmg-row__bar", attrs: { "aria-hidden": "true" } },
              h(
                "div",
                { class: "dmg-row__stack", style: { width: `${(player.damageToChampions / maxDamage) * 100}%` } },
                ...segments
                  .filter(([, value]) => value > 0)
                  .map(([kind, value, label]) =>
                    h("span", { class: `seg--${kind}`, title: `${label}: ${formatInt(value)}`, style: { "flex-grow": String(value) } }),
                  ),
              ),
            ),
            h("span", { class: "dmg-row__value", text: formatInt(player.damageToChampions) }),
          );
        }),
    ]),
  );
}

function renderPlayerTable(replay: ReplayInspection) {
  const minutes = Math.max(1 / 60, (replay.gameDurationMs ?? 0) / 60_000);
  const columns: Array<[string, (player: PlayerStats) => number, (value: number) => string]> = [
    [t("goldPerMinute"), (player) => player.goldEarned / minutes, (value) => formatInt(value)],
    [t("damageTaken"), (player) => player.damageTaken, formatInt],
    [t("damageMitigated"), (player) => player.damageSelfMitigated, formatInt],
    [t("healShield"), (player) => player.healShieldOnTeammates, formatInt],
    [t("timeDead"), (player) => player.timeDeadSeconds, formatClock],
  ];
  const maxima = columns.map(([, value]) => Math.max(...replay.players.map(value), 0));
  el.playerTable.replaceChildren(
    h(
      "div",
      { class: "ptable__row ptable__row--head", attrs: { "aria-hidden": "true" } },
      h("span", { text: t("player") }),
      ...columns.map(([label]) => h("span", { class: "num", text: label })),
    ),
    ...teamSides(replay).flatMap((side) => [
      h("div", { class: "group-label side-label", text: sideLabel(side).toUpperCase(), dataset: teamData(side) }),
      ...replay.players
        .filter((player) => player.team === side)
        .map((player) =>
          h(
            "div",
            { class: `ptable__row${player.name === state.owner ? " is-owner" : ""}` },
            h(
              "span",
              { class: "ptable__name" },
              h("span", { class: "ptable__champ", text: championName(player.champion) }),
              h("span", { class: "ptable__pos", text: POSITIONS[player.position] ?? "" }),
            ),
            ...columns.map(([label, value, format], index) => {
              const amount = value(player);
              return h(
                "div",
                { class: "cell", title: label },
                h("div", { class: "cell__track" }, h("div", { class: "cell__fill", style: { width: `${maxima[index] ? (amount / maxima[index]) * 100 : 0}%` } })),
                h("span", { class: "cell__value", text: format(amount) }),
              );
            }),
          ),
        ),
    ]),
  );
}

// ——— Archivo ———

function renderFile(replay: ReplayInspection) {
  const entry = state.library?.entries.find((item) => item.path === replay.path);
  const facts: Array<[string, string]> = [
    [t("path"), replay.path],
    [t("format"), formatLabel(replay.format)],
    [t("size"), entry ? formatBytes(entry.fileSize) : "—"],
    // Fecha del archivo (cuando se descargó o guardó), no la de la partida: igual que en el resumen.
    [t("downloaded"), formatDateLong(entry?.modifiedAtMs ?? null)],
    ["Build", replay.gameVersion ?? "—"],
    [t("region"), replay.region ?? "—"],
    [t("matchId"), replay.matchId ?? "—"],
  ];
  el.fileFacts.replaceChildren(
    ...facts.flatMap(([label, value]) => [h("dt", { text: label }), h("dd", { text: value, title: value })]),
  );
}

// ——— Panel del cliente ———

let drawerExit = 0;

/**
 * Entrada y salida del panel. Al abrir se quita `hidden` y la clase `is-open` anima la entrada; al
 * cerrar el panel sigue en el DOM, inerte, hasta que termina la salida, y recién entonces recibe
 * `hidden`. Si no corre ninguna transición (sin CSS, vista oculta) se oculta en el acto.
 */
function setDrawerPresence(open: boolean) {
  const layers = [el.scrim, el.drawer];
  if (open) {
    // La partida de detrás queda inerte; la biblioteca de al lado sigue disponible a propósito.
    setBehindInert(true);
    if (el.drawer.classList.contains("is-open")) return;
    // Reabrir durante la salida la invierte desde donde está.
    drawerExit++;
    const entering = el.drawer.hidden;
    for (const layer of layers) {
      layer.hidden = false;
      layer.inert = false;
    }
    // Desde `hidden` la entrada necesita el estilo de partida (fuera de la vista) ya calculado.
    if (entering) void el.drawer.offsetWidth;
    for (const layer of layers) layer.classList.add("is-open");
    el.drawerClose.focus({ preventScroll: true });
    return;
  }
  if (!el.drawer.classList.contains("is-open")) {
    setBehindInert(false);
    return;
  }
  const focus = document.activeElement;
  const hadFocus = el.drawer.contains(focus) || focus === document.body;
  const exit = ++drawerExit;
  for (const layer of layers) {
    layer.classList.remove("is-open");
    layer.inert = true;
  }
  // Antes de devolver el foco: un elemento inerte no lo recibe.
  setBehindInert(false);
  // El foco vuelve a la barra del cliente en vez de perderse con el panel.
  if (hadFocus) (el.clientAction.hidden || el.clientAction.disabled ? el.chooseExecutable : el.clientAction).focus();
  const finish = () => {
    if (exit !== drawerExit) return;
    for (const layer of layers) layer.hidden = true;
  };
  const running = layers.flatMap((layer) => layer.getAnimations());
  if (running.length === 0) finish();
  // También si la transición se cancela, por ejemplo al ocultarse la vista a mitad de camino.
  else void Promise.allSettled(running.map((animation) => animation.finished)).then(finish);
}

function renderDrawer() {
  const open = state.drawerOpen;
  setDrawerPresence(open);
  // El panel que sale conserva lo que mostraba hasta desaparecer.
  if (!open) return;
  const replay = state.replay;
  const install = state.drawerMode === "progress" ? state.install : null;
  const progress = !!install;
  const plan = state.plan;
  // El error de la última descarga de esta compilación sigue a la vista hasta el próximo intento.
  const version = install?.gameVersion ?? replay?.gameVersion;
  const downloadError = state.installError && state.installError.gameVersion === version ? state.installError.message : null;

  // Mientras llega el plan, el modo que se espera según el disco: el botón dice "Reanudar" desde el
  // comienzo y no cambia al llegar. Un plan de "Reanudar" que resulta nuevo (no quedó nada) se
  // titula como tal.
  const status = replay?.gameVersion ? state.statuses.get(replay.gameVersion) : undefined;
  const mode = plan?.mode ?? expectedMode(status);
  setText(el.drawerTitle, t(progress ? "downloading" : mode === "resume" ? "resume" : "prepareClient"));
  // Durante la descarga, lo que se descarga, aunque la partida abierta sea otra.
  setText(
    el.drawerSubtitle,
    install
      ? `${install.replayName} · ${replaysLabel(install.replays)}`
      : replay
        ? `${replay.fileName} · ${replaysLabel(replay.gameVersion ? replaysOf(replay.gameVersion) : 1)}`
        : "",
  );
  setText(el.planVersion, install?.build ?? plan?.gameBuild ?? replay?.gameVersion ?? "—");
  const modeText = install
    ? t("downloading")
    : plan ? modeLabel(plan.mode) : state.planning ? stageLabel(state.planStage) : "—";
  // Nueva y la descarga en curso informan; reanudar y reparar avisan; lista, en verde.
  setChip(el.planMode, modeText, install ? "info" : plan ? MODE_TONES[plan.mode] : "outline");
  const focused = document.activeElement;
  el.planView.hidden = progress;
  el.progressView.hidden = !progress;
  el.cancelClient.hidden = progress;
  el.confirmClient.hidden = progress;
  el.stopClient.hidden = !progress;
  // Al pasar al progreso, el foco de Descargar (que se oculta) queda en el panel.
  if (focused instanceof HTMLElement && focused !== el.drawer && el.drawer.contains(focused) && focused.closest("[hidden]")) {
    el.drawerClose.focus();
  }

  if (install) {
    const percent = downloadPercent(install.percent, install.done);
    // Solo el porcentaje es región viva; el archivo en curso cambia demasiado para anunciarlo.
    setText(el.progressPercent, formatDownloadPercent(percent));
    setText(
      el.progressBytes,
      install.total ? t("amountOf", { done: formatBytes(install.downloaded), total: formatBytes(install.total) }) : "",
    );
    el.progressBar.style.width = `${percent}%`;
    el.progressTrack.setAttribute("aria-valuenow", String(percent));
    // Velocidad y tiempo restante en su línea reservada, bajo los bytes: llegan tras el primer
    // segundo medido.
    setText(el.progressRate, rateText(install));
    setText(el.progressFile, install.detail);
    el.progressFile.title = install.detail;
    const current = STAGES.indexOf(install.stage);
    for (const item of el.stages) {
      const index = STAGES.indexOf(item.dataset.stage ?? "");
      item.dataset.status = index < current ? "done" : index === current ? "active" : "pending";
    }
    setText(el.progressDestination, install.destination);
    el.progressDestination.title = install.destination;
    setText(el.progressLocale, install.locale);
    // Un Detener que falló: la descarga sigue y el motivo se ve aquí y bajo la barra del cliente.
    el.planError.hidden = !downloadError;
    setText(el.planError, downloadError ?? "");
    return;
  }

  // El plan que no se pudo calcular y, debajo, el fallo de la descarga anterior.
  const planErrors = [state.planError, downloadError].filter(Boolean).join("\n");
  el.planError.hidden = !planErrors;
  setText(el.planError, planErrors);

  const destination = plan?.destinationPath ?? status?.destinationPath ?? "—";
  el.planDestination.textContent = destination;
  el.planDestination.title = destination;
  if (el.locale.value !== state.locale) {
    ensureLocale(state.locale);
    el.locale.value = state.locale;
  }
  el.locale.disabled = state.planning;
  el.planChangeRoot.disabled = state.planning;
  const pending = state.planning ? "…" : "—";
  el.planFileCount.textContent = plan
    ? t("amountOf", { done: formatInt(plan.affectedFileCount), total: formatInt(plan.fileCount) })
    : pending;
  el.planDownloadSize.textContent = plan ? formatBytes(plan.downloadBytes) : pending;
  el.planInstallSize.textContent = plan ? formatBytes(plan.totalInstallBytes) : pending;
  el.planSource.textContent = plan
    ? [plan.source === "sieve" ? "Riot Sieve" : "riot-manifests", plan.exactBuild ? null : t("notExact")].filter(Boolean).join(" · ")
    : pending;
  el.planSource.title = plan?.manifestUrl ?? "";
  el.confirmLabel.textContent = confirmLabel(mode);
  // Usar un cliente listo no descarga nada.
  el.confirmIcon.hidden = mode === "ready";
  el.confirmClient.disabled = !plan || state.planning;
}

/**
 * Modo que tendrá el plan según lo que hay en disco, para rotular el panel mientras se calcula:
 * una compilación incompleta (o un plan pedido para reanudar) se reanuda y una lista se usa.
 */
function expectedMode(status: LocalClientStatus | undefined): ClientWorkMode {
  if (state.planKind === "resume" || status?.state === "partial") return "resume";
  return status?.state === "ready" ? "ready" : "install";
}

// ——— Clientes ———

function renderClients() {
  if (document.activeElement !== el.installFolder) el.installFolder.value = state.settings.clientsRoot ?? "";
  el.revealClientsRoot.disabled = !state.settings.clientsRoot;
  // Sin elección todavía (no se preguntó), ninguna de las dos está marcada.
  for (const button of el.assetPrefetch.querySelectorAll<HTMLButtonElement>("[data-prefetch]")) {
    setAttribute(button, "aria-pressed", String(state.settings.assetPrefetch === (button.dataset.prefetch === "all")));
  }
  const error = state.clientsError ?? state.statusesError;
  el.clientsRootError.hidden = !error;
  setText(el.clientsRootError, error ?? "");
  const rows = buildRows();
  const known = rows.every((row) => row.known);
  const ready = rows.filter((row) => row.known && row.status?.state === "ready").length;
  el.clientsInstalled.textContent = rows.length && known ? `${ready}/${rows.length}` : "—";
  const bytes = rows.reduce((total, row) => total + (row.status?.installedBytes ?? 0), 0);
  el.clientsBytes.textContent = state.settings.clientsRoot && !state.statusesError ? formatBytes(bytes) : "—";
  el.buildsCount.textContent = String(rows.length);
  // La lista se rehace entera; el foco vuelve al mismo botón de la misma compilación.
  const focused = buildActionKey(document.activeElement);
  el.buildsList.replaceChildren(
    ...(rows.length
      ? rows.map(buildRow)
      : [h("p", { class: "builds__empty", text: t(state.library ? "noBuilds" : "noLibrary") })]),
  );
  if (focused) {
    const button = [...el.buildsList.querySelectorAll("button")].find((item) => buildActionKey(item) === focused);
    button?.focus({ preventScroll: true });
  }
}

/** Compilación y texto de un botón de la lista de Clientes, para reencontrarlo tras rehacerla. */
function buildActionKey(element: Element | null) {
  if (!(element instanceof HTMLButtonElement) || !el.buildsList.contains(element)) return null;
  return `${element.closest<HTMLElement>(".builds__row")?.dataset.build ?? ""}\n${element.textContent}`;
}

/** Durante una descarga solo cambia el texto de su fila: la lista no se rehace en cada evento. */
function renderInstallRow() {
  const install = state.install;
  if (!install || state.view !== "clients") return;
  for (const row of el.buildsList.querySelectorAll<HTMLElement>(".builds__row")) {
    if (row.dataset.build !== install.build) continue;
    const text = row.querySelector<HTMLElement>(".builds__state-text");
    if (text) setText(text, installStateText(install));
    const fill = row.querySelector<HTMLElement>(".builds__meter .meter__fill");
    if (fill) fill.style.width = `${downloadPercent(install.percent, install.done)}%`;
    const rate = row.querySelector<HTMLElement>(".builds__rate");
    if (rate) setText(rate, installRate(install).speed);
  }
}

function installStateText(install: InstallRun) {
  return t("downloadingPercent", { percent: formatDownloadPercent(downloadPercent(install.percent, install.done)) });
}

/** "6,1 MB/s · quedan 56 min", lo que se sepa. */
function rateText(install: InstallRun) {
  const { speed, eta } = installRate(install);
  return [speed, eta].filter(Boolean).join(" · ");
}

type BuildRow = {
  build: string;
  patch: string;
  version: string | null;
  replays: number;
  status: LocalClientStatus | null;
  /** El estado en disco ya se leyó (si no, se muestra neutro). */
  known: boolean;
};

function buildRows(): BuildRow[] {
  const rows = new Map<string, BuildRow>();
  for (const entry of state.library?.entries ?? []) {
    if (!entry.gameVersion) continue;
    const status = state.statuses.get(entry.gameVersion) ?? null;
    const build = status?.gameBuild ?? entry.gameVersion;
    const known = !state.statusesError && state.statusVersions.has(entry.gameVersion);
    const row = rows.get(build) ?? { build, patch: entry.patch ?? "—", version: entry.gameVersion, replays: 0, status, known };
    row.replays += 1;
    rows.set(build, row);
  }
  for (const status of state.installed) {
    if (!rows.has(status.gameBuild)) {
      rows.set(status.gameBuild, {
        build: status.gameBuild,
        patch: status.gameBuild.split(".").slice(0, 2).join("."),
        version: null,
        replays: 0,
        status,
        known: true,
      });
    }
  }
  return [...rows.values()].sort((left, right) => compareVersions(right.build, left.build));
}

function buildRow(row: BuildRow) {
  const install = state.install;
  const installing = install?.build === row.build;
  const statusState: GlyphState = installing ? "busy" : !row.known ? "unknown" : row.status?.state ?? "missing";
  const stateText = install && installing
    ? installStateText(install)
    : statusState === "unknown"
      ? "—"
      : statusState === "ready"
        ? t("ready")
        : statusState === "partial" && row.status
          ? partialStateText(row.status)
          : t("notInstalled");
  // Lista: lo que ocupa. Incompleta: lo que ya hay en disco (con lo guardado a medias) de su total.
  const total = row.status?.totalInstallBytes;
  const size: Array<Node | string> = statusState === "ready"
    ? [formatBytes(row.status?.installedBytes ?? 0)]
    : statusState === "partial" && total
      ? [
        h("span", { text: formatBytes(row.status?.installedBytes ?? 0) }),
        " ",
        h("small", { text: t("ofTotal", { total: formatBytes(total) }) }),
      ]
      : ["—"];
  const actions: HTMLButtonElement[] = [];
  const version = row.version;
  // Una descarga a la vez: mientras corre otra, las de esta compilación esperan.
  const busy = !!install;
  if (installing) {
    actions.push(actionButton(t("progress"), showProgress));
  } else if (row.known) {
    if (version && statusState !== "ready") {
      const kind = statusState === "partial" ? "resume" : "install";
      actions.push(actionButton(t(kind === "resume" ? "resume" : "confirmInstall"), () => void goToBuild(version, kind), true, busy));
    }
    if (version && statusState === "ready") actions.push(actionButton(t("verify"), () => void goToBuild(version, "install"), false, busy));
    const path = row.status?.state === "ready" ? row.status.executablePath : row.status?.state === "partial" ? row.status.destinationPath : null;
    if (path) actions.push(actionButton(t("open"), () => void reveal(path, "clients")));
  }
  return h(
    "div",
    { class: "builds__row", dataset: { build: row.build } },
    h(
      "div",
      { class: "builds__build" },
      h("strong", { text: row.build }),
      h("span", { text: row.status?.destinationPath ?? "", title: row.status?.destinationPath ?? "" }),
    ),
    h("span", { class: "builds__cell", text: row.patch }),
    h("span", { class: "builds__cell num", text: row.replays }),
    h("span", { class: "builds__cell", text: row.status?.locale ?? "—" }),
    h("span", { class: "builds__cell builds__size num" }, ...size),
    h(
      "span",
      { class: "builds__state", dataset: GLYPH_TONES[statusState] ? { tone: GLYPH_TONES[statusState] } : {} },
      glyphElement(statusState),
      h("span", { class: "builds__state-text", text: stateText }),
      // Progreso de la descarga bajo el estado, fuera del flujo: la fila no cambia de alto. A su
      // lado, la velocidad (el tiempo restante no cabe en la columna: está en el panel).
      install && installing
        ? h(
          "span",
          { class: "builds__progress", attrs: { "aria-hidden": "true" } },
          h(
            "span",
            { class: "meter meter--mini builds__meter" },
            h("span", { class: "meter__fill", style: { width: `${downloadPercent(install.percent, install.done)}%` } }),
          ),
          h("span", { class: "builds__rate mono", text: installRate(install).speed }),
        )
        : null,
    ),
    h("div", { class: "builds__actions" }, ...actions),
  );
}

function actionButton(label: string, run: () => void, primary = false, disabled = false) {
  return h("button", {
    class: `button button--sm${primary ? " button--primary" : ""}`,
    text: label,
    attrs: { type: "button", disabled },
    on: { click: run },
  });
}

async function goToBuild(version: string, kind: "install" | "resume") {
  const entry = state.library?.entries.find((item) => item.gameVersion === version);
  if (!entry) return;
  state.view = "library";
  renderAll();
  if (state.replay?.path !== entry.path) await selectReplay(entry.path);
  await openClientPlan(kind);
}

// ——— Utilidades ———

function championTile(championId: string, dataDragonVersion: string | null, fallbackUrl: string | null = null) {
  const name = championName(championId);
  const url = championIconUrl(championId, dataDragonVersion, fallbackUrl);
  return h("span", { class: "tile", title: name }, assetImage(url, name, abbreviate(name)));
}

function championIconUrl(
  championId: string,
  dataDragonVersion: string | null,
  fallbackUrl: string | null,
  data = state.dataDragon,
) {
  const key = normalizeKey(championId);
  const asset = data?.champions.get(key) ?? state.names?.champions.get(key);
  return asset?.iconUrl
    ?? fallbackUrl
    ?? (championId && dataDragonVersion ? `${DATA_DRAGON}/cdn/${dataDragonVersion}/img/champion/${championId}.png` : null);
}

/** Página de la partida en League of Graphs, que la encuentra por región e ID de Riot. */
function matchPageUrl(replay: ReplayInspection | null) {
  const region = replay?.region ? LEAGUE_OF_GRAPHS_REGIONS[replay.region.toUpperCase()] : undefined;
  const id = replay?.matchId;
  return region && id && /^\d+$/.test(id) ? `https://www.leagueofgraphs.com/match/${region}/${id}` : null;
}

/**
 * Imagen en una casilla de tamaño fijo. Si ya está en memoria (decodificada al leer la biblioteca)
 * se pinta en el mismo cuadro; si no, aparece con un fundido breve al llegar, sin indicador de
 * carga. Solo la lista, que puede ser larga, la pide al acercarse a la vista.
 */
function assetImage(url: string | null, label: string, fallback: string, lazy = false) {
  if (!url) return document.createTextNode(fallback);
  const image = h("img", {
    attrs: { alt: label, loading: lazy ? "lazy" : null, width: 36, height: 36, src: cachedAssetUrl(url) },
  });
  if (!image.complete) {
    image.classList.add("is-pending");
    image.addEventListener("load", () => image.classList.remove("is-pending"), { once: true });
  }
  image.addEventListener("error", () => image.replaceWith(document.createTextNode(fallback)), { once: true });
  return image;
}

function championName(championId: string) {
  const key = normalizeKey(championId);
  return state.dataDragon?.champions.get(key)?.name
    ?? state.names?.champions.get(key)?.name
    ?? championId.replace(/([a-z])([A-Z])/g, "$1 $2");
}

// La lista usa los nombres del parche más nuevo de la biblioteca, no los de la partida abierta:
// así elegir otra partida no cambia el contenido de todas las filas.
function libraryChampionName(championId: string) {
  return state.names?.champions.get(normalizeKey(championId))?.name
    ?? championId.replace(/([a-z])([A-Z])/g, "$1 $2");
}

function libraryChampionIcon(championId: string, patch: string | null) {
  return state.names?.champions.get(normalizeKey(championId))?.iconUrl
    ?? (championId && patch ? `${DATA_DRAGON}/cdn/${patch}.1/img/champion/${championId}.png` : null);
}

function abbreviate(label: string) {
  const words = label.replace(/[^\p{L}\p{N}\s]/gu, " ").split(/\s+/).filter((word) => word.length > 2 || /\d/.test(word));
  if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
  return (words[0] ?? label).slice(0, 2).toUpperCase();
}

function chip(text: string, tone: ChipTone, small = false) {
  const element = h("span");
  setChip(element, text, tone, small);
  return element;
}

/** El tono va en una clase (`is-win`, `is-loss`, `is-warn`, `is-info`), nunca según el texto. */
function setChip(element: HTMLElement, text: string, tone: ChipTone, small = false) {
  element.textContent = text;
  const accent = tone === "solid" || tone === "outline" ? "" : ` is-${tone}`;
  element.className = `chip ${tone === "solid" ? "chip--solid" : "chip--outline"}${accent}${small ? " chip--sm" : ""}`;
}

/** Tono de un estado (texto y marca); sin tono, neutro. */
function setTone(element: HTMLElement, tone: Tone | null) {
  if (tone) element.dataset.tone = tone;
  else delete element.dataset.tone;
}

/** Equipo de una etiqueta "Lado azul" o "Lado rojo": la marca de color va delante. */
function teamData(side: string): Record<string, string> {
  return side === "Blue" ? { team: "blue" } : side === "Red" ? { team: "red" } : {};
}

function setTeam(element: HTMLElement, side: string | null) {
  const team = side ? teamData(side).team : undefined;
  if (team) element.dataset.team = team;
  else delete element.dataset.team;
}

function teamSides(replay: ReplayInspection) {
  const sides = ["Blue", "Red"].filter((side) => replay.players.some((player) => player.team === side));
  const other = [...new Set(replay.players.map((player) => player.team))].filter((team) => !sides.includes(team));
  return [...sides, ...other];
}

function sideLabel(side: string) {
  return side === "Blue" ? t("blueSide") : side === "Red" ? t("redSide") : side;
}

function formatLabel(format: ReplayInspection["format"]) {
  return { rofl2: "ROFL2", classicRofl: "ROFL", unknown: "—" }[format];
}

function modeLabel(mode: ClientWorkMode) {
  const keys: Record<ClientWorkMode, MessageKey> = { install: "modeInstall", resume: "modeResume", repair: "modeRepair", ready: "modeReady" };
  return t(keys[mode]);
}

function confirmLabel(mode: ClientWorkMode) {
  const keys: Record<ClientWorkMode, MessageKey> = {
    install: "confirmInstall",
    resume: "confirmResume",
    repair: "confirmRepair",
    ready: "confirmReady",
  };
  return t(keys[mode]);
}

function stageLabel(stage: string | null) {
  const keys: Record<string, MessageKey> = { resolve: "resolving", manifest: "manifest", plan: "comparing" };
  const key = stage ? keys[stage] : undefined;
  return key ? t(key) : "…";
}

function initializeLocales() {
  el.locale.replaceChildren(...LOCALES.map(([value, label]) => h("option", { text: `${value} · ${label}`, attrs: { value } })));
}

function ensureLocale(locale: string) {
  if (![...el.locale.options].some((option) => option.value === locale)) {
    el.locale.append(h("option", { text: locale, attrs: { value: locale } }));
  }
}

function fileNameOf(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function replaysLabel(count: number) {
  return plural(count, "replaysOne", "replaysOther", formatInt(count));
}

/** Escribe el texto solo si cambió (evita volver a anunciar regiones vivas y trabajo de más). */
function setText(element: HTMLElement, text: string) {
  if (element.textContent !== text) element.textContent = text;
}

function setAttribute(element: HTMLElement, name: string, value: string) {
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}

/** Con el panel abierto, la partida (o su estado vacío) queda fuera del foco y del lector. */
function setBehindInert(inert: boolean) {
  el.workspace.inert = inert;
  el.replayEmpty.inert = inert;
}

function shortenPath(path: string) {
  if (path.length <= 46) return path;
  const parts = path.split(/[\\/]/);
  const head = parts.slice(0, 3).join("\\");
  const tail = parts.slice(-2).join("\\");
  return `${head}\\…\\${tail}`;
}

function compareVersions(left: string, right: string) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index++) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference) return difference;
  }
  return 0;
}

/** Abre la carpeta del archivo; un fallo se muestra en la vista desde la que se pidió. */
async function reveal(path: string, from: "match" | "clients") {
  const replay = from === "match" ? state.replay?.path ?? null : null;
  try {
    await revealItemInDir(path);
    if (replay && state.actionError?.path === replay) {
      state.actionError = null;
      renderClientStrip();
    } else if (!replay && state.clientsError) {
      state.clientsError = null;
      renderClients();
    }
  } catch (error) {
    // El detalle viene del sistema o del complemento: la acción fallida se nombra en el idioma de la interfaz.
    const message = t("folderOpenFailed", { error: errorText(error) });
    if (replay) {
      state.actionError = { path: replay, message };
      // Se ve el error de la última acción: el de un inicio anterior deja de mostrarse.
      if (state.launch?.state === "error") state.launch = null;
      renderClientStrip();
    } else {
      state.clientsError = message;
      renderClients();
    }
  }
}

const copyResets = new WeakMap<HTMLButtonElement, number>();

/** Copia el texto y, si se pudo, el ícono del botón pasa un momento a una marca. */
async function copyText(text: string, button: HTMLButtonElement) {
  if (!navigator.clipboard) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    return; // Sin permiso de portapapeles no hay nada que confirmar.
  }
  const glyph = button.querySelector("svg");
  if (!glyph) return;
  const className = glyph.getAttribute("class") ?? "icon";
  glyph.replaceWith(icon("check", className));
  window.clearTimeout(copyResets.get(button));
  copyResets.set(button, window.setTimeout(() => {
    button.querySelector("svg")?.replaceWith(icon("copy", className));
  }, 1500));
}

function errorText(error: unknown) {
  const detail = String(error).replace(/^Error:\s*/i, "").trim();
  return detail && detail !== "[object Object]" ? detail : t("unknownError");
}
