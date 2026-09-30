import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { h, hydrateIcons, icon, must, type IconName } from "./dom";
import {
  formatBytes,
  formatClock,
  formatDate,
  formatDateLong,
  formatDecimal,
  formatDuration,
  formatInt,
  formatPercent,
  formatThousands,
  kdaRatio,
  progressPercent,
} from "./format";
import {
  cachedAssetUrl,
  loadCommunityDragonAssets,
  loadDataDragonAssets,
  normalizeKey,
  type CommunityDragonAssets,
  type DataDragonAssets,
  type MatchAsset,
} from "./game-assets";
import type {
  AppSettings,
  ClientInstallPlan,
  ClientInstallResult,
  ClientRequest,
  ConfirmedPlan,
  ClientWorkMode,
  DownloadProgress,
  LibraryPlayer,
  LocalClientStatus,
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
  stopping: boolean;
};

// Estado de la barra del cliente para la repetición abierta.
type ClientView = {
  glyph: GlyphState;
  text: string;
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
};

type ReplayExited = { replayPath: string; exitCode: number | null };

const LAST_LIBRARY_KEY = "chronorift.lastReplayLibrary";
const GAME_EXE = "League of Legends.exe";
// Texto del backend al terminar una descarga que el usuario detuvo.
const STOPPED_MESSAGE = "Descarga detenida";
// Nota que el backend pone en toda repetición ROFL2: informa, no explica una partida sin datos.
const INFORMATIONAL_NOTES = new Set(["Metadatos ROFL2 leídos del bloque final"]);
const DATA_DRAGON = "https://ddragon.leagueoflegends.com";
// Plataforma de Riot (del nombre del archivo) → región en las URL de League of Graphs.
const LEAGUE_OF_GRAPHS_REGIONS: Record<string, string> = {
  BR1: "br", EUN1: "eune", EUW1: "euw", JP1: "jp", KR: "kr", LA1: "lan", LA2: "las", ME1: "me",
  NA1: "na", OC1: "oce", PH2: "ph", RU: "ru", SG2: "sg", TH2: "th", TR1: "tr", TW2: "tw", VN2: "vn",
};
const STAGES = ["resolve", "manifest", "plan", "download", "complete"];
const POSITIONS: Record<string, string> = {
  TOP: "TOP",
  JUNGLE: "JG",
  MIDDLE: "MID",
  BOTTOM: "BOT",
  UTILITY: "SUP",
};
// Idioma de los nombres de campeones, objetos y runas (la interfaz está en español).
const NAMES_LOCALE = (typeof navigator !== "undefined" && navigator.language?.toLowerCase() === "es-es")
  ? "es_ES"
  : "es_MX";
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

const el = {
  skipLink: must(".skip-link", HTMLAnchorElement),
  navItems: [...document.querySelectorAll<HTMLButtonElement>(".nav-item")],
  activity: must("#activity", HTMLButtonElement),
  activityLabel: must("#activity-label", HTMLElement),
  activityBuild: must("#activity-build", HTMLElement),
  activityBar: must("#activity-bar", HTMLElement),
  activityPercent: must("#activity-percent", HTMLElement),
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
  clientGlyph: must("#client-glyph", HTMLElement),
  clientText: must("#client-status-text", HTMLElement),
  clientLocation: must("#client-location", HTMLElement),
  revealClient: must("#reveal-client", HTMLButtonElement),
  clientLocale: must("#client-locale", HTMLElement),
  clientProgress: must("#client-progress", HTMLElement),
  clientProgressBar: must("#client-progress-bar", HTMLElement),
  clientError: must("#client-error", HTMLElement),
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
  progressFile: must("#progress-file", HTMLElement),
  stages: [...document.querySelectorAll<HTMLElement>("#stages li")],
  progressDestination: must("#progress-destination", HTMLElement),
  progressLocale: must("#progress-locale", HTMLElement),
  cancelClient: must("#cancel-client-action", HTMLButtonElement),
  confirmClient: must("#confirm-client-action", HTMLButtonElement),
  confirmIcon: must("#confirm-icon", HTMLElement),
  confirmLabel: must("#confirm-label", HTMLElement),
  stopClient: must("#stop-client-action", HTMLButtonElement),
  stopLabel: must("#stop-label", HTMLElement),
  clientsMain: must("#clients-content", HTMLElement),
  installFolder: must("#install-folder", HTMLInputElement),
  revealClientsRoot: must("#reveal-clients-root", HTMLButtonElement),
  clientsRootError: must("#clients-root-error", HTMLElement),
  clientsInstalled: must("#clients-installed", HTMLElement),
  clientsBytes: must("#clients-bytes", HTMLElement),
  buildsCount: must("#builds-count", HTMLElement),
  buildsList: must("#builds-list", HTMLElement),
};

hydrateIcons();
initializeLocales();
bindEvents();
renderAll();
if (import.meta.env.PROD) blockReloads();
if ("__TAURI_INTERNALS__" in window) {
  void listen<DownloadProgress>("download-progress", ({ payload }) => onProgress(payload));
  void listen<ReplayExited>("replay-exited", ({ payload }) => onReplayExited(payload));
  window.addEventListener("focus", onWindowFocus);
  bindWindowControls();
  void boot();
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
      const label = maximized ? "Restaurar" : "Maximizar";
      maximize.replaceChildren(icon(maximized ? "restore" : "maximize"));
      maximize.setAttribute("aria-label", label);
      maximize.title = label;
    } catch {
      // Sin estado de la ventana se conserva el icono actual.
    }
  };
  void sync();
  current.onResized(() => void sync()).catch(() => undefined);
}

async function boot() {
  void refreshVanguard();
  await loadSettings();
  const saved = localStorage.getItem(LAST_LIBRARY_KEY);
  if (saved) await loadLibrary(saved);
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
  el.activity.addEventListener("click", showProgress);
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
    state.owner = detectOwner(library.entries);
    if (!state.owner) state.filter = "all";
    localStorage.setItem(LAST_LIBRARY_KEY, library.folderPath);
    void refreshClientStatuses();
    void loadChampionNames(library);
    // La selección vigente al terminar el escaneo, no la del momento del clic: pudo cambiar.
    const target = library.entries.find((entry) => entry.path === state.selectedPath)
      ?? visibleEntries()[0]
      ?? library.entries[0];
    if (target) void selectReplay(target.path);
    else clearSelection();
  } catch (error) {
    state.library = null;
    state.libraryPath = path;
    state.owner = null;
    state.libraryError = errorText(error);
    // La lectura previa de la carpeta anterior deja de servir.
    state.libraryRun++;
    state.inspections.clear();
    clearSelection();
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
  state.dataDragon = ready.data;
  state.communityDragon = ready.community;
  renderAll();
  if (!ready.data || !ready.community) void enrichMatch(replay);
}

/**
 * Lee de antemano cada repetición de la biblioteca y los datos de cada parche, en el orden de la
 * lista, para que elegir una partida no tenga que esperar nada.
 */
async function prefetchLibrary(library: ReplayLibrary) {
  const run = ++state.libraryRun;
  const patches = new Set(library.entries.map((entry) => entry.patch).filter((patch): patch is string => !!patch));
  for (const patch of patches) {
    dataDragon(`${patch}.1`).catch(() => undefined);
    communityDragon(patch).catch(() => undefined);
  }
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
}

function assetKeys(replay: ReplayInspection) {
  return {
    data: `${replay.dataDragonVersion ?? `${replay.patch}.1`}:${NAMES_LOCALE}`,
    community: `${replay.patch}:${NAMES_LOCALE}`,
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
  const key = `${version}:${NAMES_LOCALE}`;
  let entry = dataDragonCache.get(key);
  if (!entry) {
    entry = loadDataDragonAssets(version, NAMES_LOCALE);
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
  const key = `${patch}:${NAMES_LOCALE}`;
  let entry = communityDragonCache.get(key);
  if (!entry) {
    entry = loadCommunityDragonAssets(patch, NAMES_LOCALE);
    communityDragonCache.set(key, entry);
    entry.then(
      (assets) => readyCommunityDragon.set(key, assets),
      () => communityDragonCache.delete(key),
    );
  }
  return entry;
}

async function enrichMatch(replay: ReplayInspection) {
  if (replay.status !== "parsed" || !replay.patch) return;
  const run = ++state.assetRun;
  const [data, community] = await Promise.allSettled([
    dataDragon(replay.dataDragonVersion ?? `${replay.patch}.1`),
    communityDragon(replay.patch),
  ]);
  if (run !== state.assetRun || state.replay?.path !== replay.path) return;
  state.dataDragon = data.status === "fulfilled" ? data.value : null;
  state.communityDragon = community.status === "fulfilled" ? community.value : null;
  renderMatchBody();
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
  state.install = {
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
    stopping: false,
  };
  state.drawerMode = "progress";
  dropPlan();
  renderAll();
  let failed = false;
  try {
    const result = await invoke<ClientInstallResult>("install_client", { request });
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
    state.install = null;
    state.drawerOpen = false;
    // Solo pudo quedar el aviso de un Detener que falló durante esta misma descarga.
    state.installError = null;
  } catch (error) {
    const message = errorText(error);
    const run = state.install;
    // Solo es una parada del usuario si pidió Detener y el backend terminó por eso.
    const stopped = run?.stopping === true && message === STOPPED_MESSAGE;
    state.install = null;
    state.drawerMode = "plan";
    if (stopped) {
      state.drawerOpen = false;
    } else {
      failed = true;
      state.installError = { gameVersion: version, build: run?.build ?? plan.gameBuild, message };
      state.planKind = "resume";
      // El panel abierto seguía esta descarga: la barra pasa a su repetición para mostrar el fallo.
      if (state.drawerOpen && run && state.replay?.gameVersion !== version && state.inspections.has(run.replayPath)) {
        void selectReplay(run.replayPath);
      }
    }
  }
  renderAll();
  await refreshClientStatuses();
  // Tras un fallo el plan se rehace solo, así Reanudar queda listo con un clic.
  if (failed && !state.install && !state.planning && !state.plan && state.replay?.gameVersion === version) {
    state.locale = clientLocale();
    void refreshPlan();
  }
}

async function stopInstall() {
  const install = state.install;
  if (!install || install.stopping) return;
  install.stopping = true;
  renderInstall();
  try {
    await invoke("cancel_client_install");
  } catch (error) {
    if (state.install !== install) return;
    install.stopping = false;
    state.installError = { gameVersion: install.gameVersion, build: install.build, message: errorText(error) };
    renderAll();
  }
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
    }
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
    state.actionError = { path: replay.path, message: `No es ${GAME_EXE}: ${selected}` };
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
    build: version ?? "—",
    inexact: false,
    locationMuted: false,
    reveal: null,
    action: null,
    executable: null,
    progress: null,
    locale: clientLocale(),
    error: null,
  };
  let view: ClientView;

  if (install && install.gameVersion === version) {
    const percent = progressPercent(install.percent, install.done);
    view = {
      ...none,
      glyph: "busy",
      text: installStateText(install),
      build: install.build,
      inexact: !install.exactBuild,
      location: install.destination,
      reveal: install.destination,
      action: { label: "Progreso", icon: "download", run: showProgress },
      progress: percent,
    };
  } else if (manual) {
    view = { ...none, glyph: "ready", text: "Ejecutable elegido", location: manual, reveal: manual, executable: manual };
  } else if (replay.status !== "parsed" || !replay.patch) {
    view = { ...none, glyph: "missing", text: "Sin build", location: "—", locationMuted: true, locale: "—" };
  } else if (!state.settings.clientsRoot) {
    view = {
      ...none,
      glyph: "missing",
      text: "Sin carpeta de clientes",
      location: "—",
      locationMuted: true,
      action: { label: "Elegir carpeta", icon: "folder", run: () => void chooseClientsRoot() },
    };
  } else if (state.statusesError) {
    // Carpeta de clientes inaccesible: no se sabe si hay cliente, así que no se ofrece nada.
    view = { ...none, glyph: "unknown", text: "No disponible", location: "—", locationMuted: true, error: state.statusesError };
  } else if (!version || !state.statusVersions.has(version)) {
    // Estado todavía sin leer: neutro, ni "Listo" ni "No instalado".
    view = { ...none, glyph: "unknown", text: "—", location: "—", locationMuted: true };
  } else if (status?.state === "ready" && status.executablePath) {
    const executable = status.executablePath;
    view = {
      ...none,
      glyph: "ready",
      text: "Listo",
      build: status.gameBuild,
      inexact: status.exactBuild === false,
      location: status.destinationPath,
      reveal: executable,
      executable,
    };
  } else if (status?.state === "partial") {
    const percent = status.totalInstallBytes
      ? progressPercent((status.installedBytes / status.totalInstallBytes) * 100, false)
      : null;
    view = {
      ...none,
      glyph: "partial",
      text: `Incompleto${percent == null ? "" : ` · ${formatPercent(percent)}`}`,
      build: status.gameBuild,
      inexact: status.exactBuild === false,
      location: status.destinationPath,
      reveal: status.destinationPath,
      action: { label: "Reanudar", icon: "download", ...plan("resume") },
    };
  } else {
    view = {
      ...none,
      glyph: "missing",
      text: "No instalado",
      location: status?.destinationPath ?? "—",
      locationMuted: true,
      action: { label: "Preparar cliente", icon: "download", ...plan("install") },
    };
  }

  const launch = state.launch?.path === replay.path ? state.launch : null;
  if (launch?.state === "launching") view.text = "Abriendo…";
  else if (view.executable && state.vanguard === "running") {
    // Con Vanguard activo el juego no abre: eso manda sobre "Reproduciendo".
    view.glyph = "blocked";
    view.text = "Vanguard activo";
  } else if (view.executable && isPlaying(replay.path)) view.text = "Reproduciendo";
  else if (launch?.state === "error") view.text = "No se pudo reproducir";

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
  return view;
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
  const install = state.install;
  el.activity.hidden = !install;
  if (install) {
    const percent = progressPercent(install.percent, install.done);
    setText(el.activityLabel, install.stopping ? "Deteniendo…" : "Descargando");
    setText(el.activityBuild, install.build);
    el.activityBar.style.width = `${percent}%`;
    setText(el.activityPercent, formatPercent(percent));
  }
  el.storageRoot.textContent = state.settings.clientsRoot ?? "Sin carpeta";
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
    ? "Cargando…"
    : state.libraryError
      ? `No se pudo cargar la carpeta. ${state.libraryError}`
      : !library
        ? "Sin carpeta"
        : total === 0
          ? "Sin repeticiones"
          : "Sin resultados";

  el.libraryOwner.hidden = !state.owner;
  el.libraryOwnerName.textContent = state.owner ?? "";
  el.libraryOwnerName.title = state.owner ?? "";
  const versions = new Set(library?.entries.map((entry) => entry.gameVersion).filter(Boolean) as string[]);
  const known = [...versions].every((version) => state.statusVersions.has(version));
  const ready = [...versions].filter((version) => state.statuses.get(version)?.state === "ready").length;
  const failures = library?.failures.length ? ` · ${library.failures.length} con error` : "";
  // La cuenta de builds listos espera a que se lean los estados.
  el.librarySummary.textContent = library && state.settings.clientsRoot && versions.size && known
    ? `${ready}/${versions.size} builds${failures}`
    : failures.replace(/^ · /, "");
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
  const content = [owner?.name ?? "", name, kda, portrait ?? ""].join("\n");
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
      owner ? (owner.win ? "Victoria" : "Derrota") : null,
      name,
      parsed ? (scored ? kda : null) : "no válida",
      entry.gameDurationMs == null ? null : formatDuration(entry.gameDurationMs),
      entry.patch ? `parche ${entry.patch}` : null,
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
        ? h("span", { class: `result-badge result-badge--${owner.win ? "win" : "loss"}`, text: owner.win ? "V" : "D" })
        : null,
    ),
    owner
      ? h("span", { class: "tile", title: name }, assetImage(portrait, name, abbreviate(name)))
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
  return {
    ready: "cliente listo",
    partial: "cliente incompleto",
    missing: "cliente no instalado",
    busy: "descargando",
    blocked: "Vanguard activo",
    unknown: null,
  }[glyph];
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
      ? "Cargando…"
      : state.replayError
        ? "No se pudo abrir la repetición"
        : "Sin repetición seleccionada";
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
    setChip(el.resultChip, owner.win ? "Victoria" : "Derrota", owner.win);
    el.resultSide.textContent = owner.team === "Blue" ? "Lado azul" : owner.team === "Red" ? "Lado rojo" : "";
  } else {
    setChip(el.resultChip, blueWon ? "Gana azul" : redWon ? "Gana rojo" : "Sin resultado", blueWon || redWon);
    el.resultSide.textContent = "";
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
    scored ? `${el.resultChip.textContent}. Azul ${replay.blueKills}, rojo ${replay.redKills}` : el.resultChip.textContent ?? "",
  );
  el.duration.textContent = formatDuration(replay.gameDurationMs);
  el.patch.textContent = replay.patch ?? "—";
  el.region.textContent = replay.region ?? "—";
  const matchPage = matchPageUrl(replay);
  el.matchId.replaceChildren(replay.matchId ?? "—", matchPage ? icon("external") : "");
  el.matchId.disabled = !matchPage;
  el.matchId.title = matchPage ? "Abrir en League of Graphs" : "";
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
  setText(el.clientText, view.text);
  setText(el.version, view.build);
  el.version.title = view.inexact && state.replay?.gameVersion ? `Repetición: ${state.replay.gameVersion}` : "";
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
  el.clientError.hidden = !view.error;
  setText(el.clientError, view.error ?? "");
  el.clientError.title = view.error ?? "";
}

// ——— Marcador ———

/** Sin jugadores: una repetición que no se pudo analizar dice por qué (notas del backend). */
function emptyMatchText(replay: ReplayInspection) {
  // Las notas explican una lectura fallida o estadísticas omitidas (más de 32 jugadores); la que
  // solo dice de dónde salieron los metadatos no explica nada.
  const reasons = replay.notes.filter((note) => !INFORMATIONAL_NOTES.has(note));
  return reasons.length ? reasons.join(" ") : "Sin estadísticas";
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
        { class: "sb-team", attrs: { "aria-label": `${label}, ${win ? "victoria" : "derrota"}` } },
        h(
          "div",
          { class: "sb-team__head" },
          h("h3", { text: label }),
          chip(win ? "Victoria" : "Derrota", win, true),
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
    ["Asesinatos", String(team.kills), "objectives__kills"],
    ["Torres", String(team.towers)],
    ["Inhib.", String(team.inhibitors)],
    ["Dragones", String(team.dragons)],
    ["Barones", String(team.barons)],
    ["Heraldos", String(team.heralds)],
    ["Larvas", String(team.voidgrubs)],
  ];
  if (team.atakhan) pairs.push(["Atakhan", String(team.atakhan)]);
  pairs.push(["Oro", formatThousands(team.goldEarned)]);
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
        "aria-label": `${player.name || "Jugador"}, ${champion}, ${player.kills}/${player.deaths}/${player.assists}`,
      },
    },
    h(
      "div",
      { class: "sb-champ" },
      championTile(player.champion, replay.dataDragonVersion, player.championImageUrl),
      player.level ? h("span", { class: "level", title: "Nivel", text: player.level }) : null,
    ),
    h(
      "div",
      { class: "sb-name" },
      h(
        "div",
        { class: "sb-name__line" },
        h("span", { class: "sb-name__id", text: player.gameName || player.name || "—", title: player.name }),
        player.tagLine ? h("span", { class: "sb-name__tag", text: `#${player.tagLine}` }) : null,
        owner ? h("span", { class: "owner-tag", text: "Tú" }) : null,
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

function loadout(player: PlayerStats) {
  const spells = state.communityDragon?.spells;
  const perks = state.communityDragon?.perks;
  const styles = state.communityDragon?.styles;
  const slots: Array<[string | null, Map<string, MatchAsset> | undefined, boolean]> = [
    [player.summonerSpells[0] ?? null, spells, false],
    [player.summonerSpells[1] ?? null, spells, false],
    [player.keystone ?? player.perks[0] ?? null, perks, true],
    [player.subStyle, styles, true],
  ];
  return slots.map(([id, assets, round]) => {
    const asset = id ? assets?.get(id) : undefined;
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

// Inventario (0–5), objeto de la misión de rol (7) si la partida lo usa y abalorio (6).
function items(player: PlayerStats, roleSlot: boolean) {
  const slots = [0, 1, 2, 3, 4, 5, ...(roleSlot ? [7] : []), 6];
  return slots.map((slot) => {
    const item = player.items.find((candidate) => candidate.slot === slot);
    const asset = item ? state.dataDragon?.items.get(item.id) : undefined;
    const label = item ? asset?.name ?? `Objeto ${item.id}` : "";
    const classes = ["slot", "slot--item"];
    if (slot === 7) classes.push("slot--role");
    if (!item) classes.push("slot--empty");
    if (slot === 6) classes.push("slot--trinket");
    return h(
      "span",
      { class: classes.join(" "), title: label },
      item ? assetImage(asset?.iconUrl ?? item.imageUrl, label, abbreviate(asset?.name ?? item.id)) : null,
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
    ["Asesinatos", blue.kills, red.kills, String],
    ["Oro", blue.goldEarned, red.goldEarned, formatThousands],
    ["Daño a campeones", blue.damageToChampions, red.damageToChampions, formatThousands],
    ["Torres", blue.towers, red.towers, String],
    ["Inhibidores", blue.inhibitors, red.inhibitors, String],
    ["Dragones", blue.dragons, red.dragons, String],
    ["Barones", blue.barons, red.barons, String],
    ["Heraldos", blue.heralds, red.heralds, String],
    ["Larvas del Vacío", blue.voidgrubs, red.voidgrubs, String],
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
          { class: "compare-row", attrs: { role: "group", "aria-label": `${label}: azul ${format(left)}, rojo ${format(right)}` } },
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
      h("div", { class: "group-label", text: sideLabel(side).toUpperCase() }),
      ...replay.players
        .filter((player) => player.team === side)
        .map((player) => {
          const owner = player.name === state.owner;
          const segments: Array<[string, number, string]> = [
            ["physical", player.physicalDamageToChampions, "Físico"],
            ["magic", player.magicDamageToChampions, "Mágico"],
            ["true", player.trueDamageToChampions, "Verdadero"],
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
    ["Oro/min", (player) => player.goldEarned / minutes, (value) => formatInt(value)],
    ["Daño recibido", (player) => player.damageTaken, formatInt],
    ["Daño mitigado", (player) => player.damageSelfMitigated, formatInt],
    ["Curación y escudos", (player) => player.healShieldOnTeammates, formatInt],
    ["Tiempo muerto", (player) => player.timeDeadSeconds, formatClock],
  ];
  const maxima = columns.map(([, value]) => Math.max(...replay.players.map(value), 0));
  el.playerTable.replaceChildren(
    h(
      "div",
      { class: "ptable__row ptable__row--head", attrs: { "aria-hidden": "true" } },
      h("span", { text: "Jugador" }),
      ...columns.map(([label]) => h("span", { class: "num", text: label })),
    ),
    ...teamSides(replay).flatMap((side) => [
      h("div", { class: "group-label", text: sideLabel(side).toUpperCase() }),
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
    ["Ruta", replay.path],
    ["Formato", formatLabel(replay.format)],
    ["Tamaño", entry ? formatBytes(entry.fileSize) : "—"],
    // Fecha del archivo (cuando se descargó o guardó), no la de la partida: igual que en el resumen.
    ["Descargada", formatDateLong(entry?.modifiedAtMs ?? null)],
    ["Build", replay.gameVersion ?? "—"],
    ["Región", replay.region ?? "—"],
    ["ID de partida", replay.matchId ?? "—"],
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

  // Un plan de "Reanudar" que resulta nuevo (no quedó nada) se titula como tal.
  const resume = plan ? plan.mode === "resume" : state.planKind === "resume";
  setText(el.drawerTitle, progress ? "Descargando" : resume ? "Reanudar" : "Preparar cliente");
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
  const mode = install
    ? install.stopping ? "Deteniendo" : "Descargando"
    : plan ? modeLabel(plan.mode) : state.planning ? stageLabel(state.planStage) : "—";
  setChip(el.planMode, mode, false);
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
    const percent = progressPercent(install.percent, install.done);
    // Solo el porcentaje es región viva; el archivo en curso cambia demasiado para anunciarlo.
    setText(el.progressPercent, formatPercent(percent));
    setText(
      el.progressBytes,
      install.total ? `${formatBytes(install.downloaded)} de ${formatBytes(install.total)}` : "",
    );
    el.progressBar.style.width = `${percent}%`;
    el.progressTrack.setAttribute("aria-valuenow", String(percent));
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
    el.stopClient.disabled = install.stopping;
    setText(el.stopLabel, install.stopping ? "Deteniendo…" : "Detener");
    // Un Detener que falló: la descarga sigue y el motivo se ve aquí y bajo la barra del cliente.
    el.planError.hidden = !downloadError;
    setText(el.planError, downloadError ?? "");
    return;
  }

  // El plan que no se pudo calcular y, debajo, el fallo de la descarga anterior.
  const planErrors = [state.planError, downloadError].filter(Boolean).join("\n");
  el.planError.hidden = !planErrors;
  setText(el.planError, planErrors);

  const status = replay?.gameVersion ? state.statuses.get(replay.gameVersion) : undefined;
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
  el.planFileCount.textContent = plan ? `${formatInt(plan.affectedFileCount)} de ${formatInt(plan.fileCount)}` : pending;
  el.planDownloadSize.textContent = plan ? formatBytes(plan.downloadBytes) : pending;
  el.planInstallSize.textContent = plan ? formatBytes(plan.totalInstallBytes) : pending;
  el.planSource.textContent = plan
    ? [plan.source === "sieve" ? "Riot Sieve" : "riot-manifests", plan.exactBuild ? null : "no exacta"].filter(Boolean).join(" · ")
    : pending;
  el.planSource.title = plan?.manifestUrl ?? "";
  el.confirmLabel.textContent = plan ? confirmLabel(plan.mode) : "Descargar";
  // Usar un cliente listo no descarga nada.
  el.confirmIcon.hidden = plan?.mode === "ready";
  el.confirmClient.disabled = !plan || state.planning;
}

// ——— Clientes ———

function renderClients() {
  if (document.activeElement !== el.installFolder) el.installFolder.value = state.settings.clientsRoot ?? "";
  el.revealClientsRoot.disabled = !state.settings.clientsRoot;
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
      : [h("p", { class: "builds__empty", text: state.library ? "Sin builds" : "Sin biblioteca" })]),
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
    const text = row.dataset.build === install.build ? row.querySelector<HTMLElement>(".builds__state-text") : null;
    if (text) setText(text, installStateText(install));
  }
}

function installStateText(install: InstallRun) {
  return install.stopping ? "Deteniendo…" : `Descargando · ${formatPercent(progressPercent(install.percent, install.done))}`;
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
  const percent = row.status?.totalInstallBytes
    ? progressPercent((row.status.installedBytes / row.status.totalInstallBytes) * 100, false)
    : null;
  const stateText = install && installing
    ? installStateText(install)
    : statusState === "unknown"
      ? "—"
      : statusState === "ready"
        ? "Listo"
        : statusState === "partial"
          ? `Incompleto${percent == null ? "" : ` · ${formatPercent(percent)}`}`
          : "No instalado";
  const size = statusState === "ready"
    ? formatBytes(row.status?.installedBytes ?? 0)
    : row.status?.totalInstallBytes
      ? formatBytes(row.status.totalInstallBytes)
      : "—";
  const actions: HTMLButtonElement[] = [];
  const version = row.version;
  // Una descarga a la vez: mientras corre otra, las de esta compilación esperan.
  const busy = !!install;
  if (installing) {
    actions.push(actionButton("Progreso", showProgress));
  } else if (row.known) {
    if (version && statusState !== "ready") {
      const kind = statusState === "partial" ? "resume" : "install";
      actions.push(actionButton(kind === "resume" ? "Reanudar" : "Descargar", () => void goToBuild(version, kind), true, busy));
    }
    if (version && statusState === "ready") actions.push(actionButton("Verificar", () => void goToBuild(version, "install"), false, busy));
    const path = row.status?.state === "ready" ? row.status.executablePath : row.status?.state === "partial" ? row.status.destinationPath : null;
    if (path) actions.push(actionButton("Abrir", () => void reveal(path, "clients")));
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
    h("span", { class: "builds__cell num", text: size }),
    h(
      "span",
      { class: "builds__state" },
      glyphElement(statusState),
      h("span", { class: "builds__state-text", text: stateText }),
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
  const asset = state.dataDragon?.champions.get(normalizeKey(championId))
    ?? state.names?.champions.get(normalizeKey(championId));
  const url = asset?.iconUrl
    ?? fallbackUrl
    ?? (championId && dataDragonVersion ? `${DATA_DRAGON}/cdn/${dataDragonVersion}/img/champion/${championId}.png` : null);
  return h("span", { class: "tile", title: name }, assetImage(url, name, abbreviate(name)));
}

/** Página de la partida en League of Graphs, que la encuentra por región e ID de Riot. */
function matchPageUrl(replay: ReplayInspection | null) {
  const region = replay?.region ? LEAGUE_OF_GRAPHS_REGIONS[replay.region.toUpperCase()] : undefined;
  const id = replay?.matchId;
  return region && id && /^\d+$/.test(id) ? `https://www.leagueofgraphs.com/match/${region}/${id}` : null;
}

function assetImage(url: string | null, label: string, fallback: string) {
  if (!url) return document.createTextNode(fallback);
  const image = h("img", { attrs: { src: cachedAssetUrl(url), alt: label, loading: "lazy", width: 36, height: 36 } });
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

function chip(text: string, solid: boolean, small = false) {
  const element = h("span");
  setChip(element, text, solid, small);
  return element;
}

function setChip(element: HTMLElement, text: string, solid: boolean, small = false) {
  element.textContent = text;
  element.className = `chip ${solid ? "chip--solid" : "chip--outline"}${small ? " chip--sm" : ""}`;
}

function teamSides(replay: ReplayInspection) {
  const sides = ["Blue", "Red"].filter((side) => replay.players.some((player) => player.team === side));
  const other = [...new Set(replay.players.map((player) => player.team))].filter((team) => !sides.includes(team));
  return [...sides, ...other];
}

function sideLabel(side: string) {
  return side === "Blue" ? "Lado azul" : side === "Red" ? "Lado rojo" : side;
}

function formatLabel(format: ReplayInspection["format"]) {
  return { rofl2: "ROFL2", classicRofl: "ROFL", unknown: "—" }[format];
}

function modeLabel(mode: ClientWorkMode) {
  return { install: "Nueva", resume: "Reanudar", repair: "Reparar", ready: "Lista" }[mode];
}

function confirmLabel(mode: ClientWorkMode) {
  return { install: "Descargar", resume: "Reanudar", repair: "Reparar", ready: "Usar cliente" }[mode];
}

function stageLabel(stage: string | null) {
  if (!stage) return "…";
  return { resolve: "Resolviendo", manifest: "Manifiesto", plan: "Comparando" }[stage] ?? "…";
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
  return `${count} ${count === 1 ? "repetición" : "repeticiones"}`;
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
    // El detalle viene del sistema o del complemento: la acción fallida se nombra en español.
    const message = `No se pudo abrir la carpeta. ${errorText(error)}`;
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
  return detail && detail !== "[object Object]" ? detail : "Error desconocido";
}
