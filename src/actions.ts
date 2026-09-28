// User-triggered actions. Nothing in here downloads, installs, or launches a
// game client unless it is the direct handler of an explicit button.

import {
  api,
  errorMessage,
  isTauri,
  knownRegion,
  onDownloadProgress,
  pickFile,
  pickFolder,
  toCommandError,
  type ClientInstallRequest,
  type InstalledClient,
} from "./api";
import { loadAssets, loadChampionIndex } from "./datadragon";
import { isValidLocale, joinPath } from "./format";
import { idleInstall, isInstallBusy, notify, setStatus, state, type InstallOp } from "./state";

export async function init() {
  void onDownloadProgress((progress) => {
    const phase = state.install.phase;
    if (phase !== "installing" && phase !== "repairing") return;
    state.install.progress = progress;
    notify("client");
  }).catch((error) => setStatus(`Progress events unavailable: ${errorMessage(error)}`, true));

  try {
    state.settings = await api.loadSettings();
  } catch (error) {
    setStatus(`Could not load settings: ${errorMessage(error)}`, true);
  }
  state.locale = state.settings.locale ?? "en_US";
  state.executablePath = state.settings.executablePath ?? "";
  notify("settings", "client");
  void restoreWindowSize();

  if (state.settings.clientsRoot) void refreshClients();
  if (state.settings.replayFolder) await scan();
  const last = state.settings.lastReplay;
  if (last && state.library?.replays.some((replay) => replay.path === last)) {
    await selectReplay(last);
  }
  if (!isTauri) await applyPreviewDemo();
}

// Window size ------------------------------------------------------------

const RESIZE_SAVE_DELAY_MS = 600;

/** Restores the saved inner size, then saves later resizes (debounced). Desktop only. */
async function restoreWindowSize() {
  if (!isTauri) return;
  try {
    const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    const { windowWidth, windowHeight } = state.settings;
    if (windowWidth && windowHeight) await win.setSize(new LogicalSize(windowWidth, windowHeight));

    let timer: number | undefined;
    await win.onResized(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(async () => {
        try {
          if (await win.isMaximized() || await win.isMinimized()) return;
          const size = (await win.innerSize()).toLogical(await win.scaleFactor());
          const width = Math.round(size.width);
          const height = Math.round(size.height);
          if (width === state.settings.windowWidth && height === state.settings.windowHeight) return;
          state.settings.windowWidth = width;
          state.settings.windowHeight = height;
          await persistSettings();
        } catch {
          // Size is a convenience; never bother the user about it.
        }
      }, RESIZE_SAVE_DELAY_MS);
    });
  } catch (error) {
    setStatus(`Could not restore window size: ${errorMessage(error)}`, true);
  }
}

/** Browser preview only: `?demo=plan|inexact|repair|progress|localeUnavailable` shows a static state. */
async function applyPreviewDemo() {
  const kind = new URLSearchParams(window.location.search).get("demo");
  if (!kind) return;
  const { demoInstall } = await import("./mock");
  const demo = demoInstall(kind);
  if (!demo) return;
  await selectReplay(demo.replayPath);
  if (demo.locale) setLocale(demo.locale);
  state.install = { ...idleInstall(), ...demo.install };
  notify("client");
}

async function persistSettings() {
  try {
    state.settings = await api.saveSettings(state.settings);
  } catch (error) {
    setStatus(`Could not save settings: ${errorMessage(error)}`, true);
  }
  notify("settings");
}

// Library ----------------------------------------------------------------

export async function scan() {
  const folder = state.settings.replayFolder;
  if (!folder) return;
  state.libraryLoading = true;
  state.libraryError = null;
  notify("library");
  try {
    const library = await api.scanReplayFolder(folder);
    if (state.settings.replayFolder !== folder) return;
    state.library = library;
    const count = library.replays.length;
    setStatus(`${count} replay${count === 1 ? "" : "s"} in folder`);
    refreshChampionIndex();
  } catch (error) {
    state.library = null;
    state.libraryError = errorMessage(error);
  } finally {
    state.libraryLoading = false;
    notify("library");
  }
}

/** Champion names for library icons and the filter, in the selected locale. Offline: ids only. */
let championIndexKey = "";

function refreshChampionIndex() {
  const library = state.library;
  if (!library?.replays.some((replay) => replay.blueChampions.length || replay.redChampions.length)) return;
  const locale = isValidLocale(state.locale) ? state.locale : "en_US";
  const key = `${library.folder}:${locale}`;
  if (key === championIndexKey && state.libraryChampions) return;
  void loadChampionIndex(locale)
    .then((index) => {
      if (state.library !== library) return;
      state.libraryChampions = index;
      championIndexKey = key;
      notify("library");
    })
    .catch(() => {});
}

export async function changeReplayFolder() {
  try {
    const folder = await pickFolder("Choose replay folder");
    if (!folder) return;
    state.settings.replayFolder = folder;
    state.filter = "";
    notify("settings");
    await persistSettings();
    await scan();
  } catch (error) {
    setStatus(errorMessage(error), true);
  }
}

export async function openReplayFile() {
  try {
    const path = await pickFile("Open replay", { name: "League replay", extensions: ["rofl"] });
    if (path) await selectReplay(path);
  } catch (error) {
    setStatus(errorMessage(error), true);
  }
}

export function setFilter(value: string) {
  state.filter = value;
  notify("library");
}

// Replay -----------------------------------------------------------------

export async function selectReplay(path: string) {
  if (state.selectedPath === path && (state.replay || state.replayLoading)) return;
  state.selectedPath = path;
  state.replay = null;
  state.replayError = null;
  state.replayLoading = true;
  state.assets = null;
  state.assetsError = null;
  if (!isInstallBusy(state.install.phase)) resetInstall();
  notify("selection", "replay", "assets", "client");
  if (state.settings.lastReplay !== path) {
    state.settings.lastReplay = path;
    void persistSettings();
  }

  try {
    const replay = await api.inspectReplay(path);
    if (state.selectedPath !== path) return;
    state.replay = replay;
    if (!state.settings.locale && replay.suggestedLocale && isValidLocale(replay.suggestedLocale)) {
      // Default only: the field stays visible and editable.
      state.locale = replay.suggestedLocale;
      refreshChampionIndex();
    }
    if (!isInstallBusy(state.install.phase)) state.region = knownRegion(replay.region);
    updateDefaultDestination();
    void refreshAssets();
  } catch (error) {
    if (state.selectedPath !== path) return;
    state.replayError = errorMessage(error);
  } finally {
    if (state.selectedPath === path) {
      state.replayLoading = false;
      notify("replay", "client");
    }
  }
}

export const STATIC_DATA_UNAVAILABLE = "Static data unavailable (offline?)";

async function refreshAssets() {
  const replay = state.replay;
  if (!replay || replay.status !== "parsed") return;
  const version = replay.dataDragonVersion ?? replay.gameVersion ?? replay.patch;
  if (!version) return;
  const locale = isValidLocale(state.locale) ? state.locale : "en_US";
  try {
    const assets = await loadAssets(version, locale);
    if (state.replay !== replay) return;
    state.assets = assets;
    state.assetsError = null;
  } catch (error) {
    if (state.replay !== replay) return;
    // One quiet note; the views fall back to text. Details go to the console only.
    console.warn("DataDragon unavailable", error);
    state.assetsError = STATIC_DATA_UNAVAILABLE;
  }
  notify("assets");
}

// Client -----------------------------------------------------------------

export function setLocale(value: string) {
  state.locale = value.trim();
  invalidatePlan();
  if (isValidLocale(state.locale)) {
    updateDefaultDestination();
    state.settings.locale = state.locale;
    void persistSettings();
    void refreshAssets();
    refreshChampionIndex();
  }
  notify("client");
}

export function useSuggestedLocale() {
  const suggested = state.replay?.suggestedLocale;
  if (suggested) setLocale(suggested);
}

export function setRegion(value: string) {
  state.region = knownRegion(value);
  invalidatePlan();
  notify("client");
}

export function setDestination(value: string) {
  state.destination = value;
  state.destinationEdited = value.trim() !== "";
  invalidatePlan();
  if (!state.destinationEdited) updateDefaultDestination();
  notify("client");
}

export async function chooseDestination() {
  try {
    const folder = await pickFolder("Choose install destination");
    if (folder) setDestination(folder);
  } catch (error) {
    setStatus(errorMessage(error), true);
  }
}

/** Updates the path while typing; pass `persist` once editing is done. */
export function setExecutablePath(value: string, persist = true) {
  state.executablePath = value.trim();
  state.launchError = null;
  if (persist) {
    state.settings.executablePath = state.executablePath || null;
    void persistSettings();
  }
  notify("client");
}

export async function chooseExecutable() {
  try {
    const path = await pickFile("Choose League of Legends executable", {
      name: "League executable",
      extensions: ["exe"],
    });
    if (path) setExecutablePath(path);
  } catch (error) {
    setStatus(errorMessage(error), true);
  }
}

export async function changeClientsRoot() {
  try {
    const folder = await pickFolder("Choose clients folder");
    if (!folder) return;
    state.settings.clientsRoot = folder;
    await persistSettings();
    updateDefaultDestination();
    notify("client");
    await refreshClients();
  } catch (error) {
    setStatus(errorMessage(error), true);
  }
}

export async function refreshClients() {
  const root = state.settings.clientsRoot;
  if (!root) {
    state.clients = [];
    notify("clients");
    return;
  }
  try {
    state.clients = await api.findInstalledClients(root);
    state.clientsError = null;
  } catch (error) {
    state.clients = [];
    state.clientsError = errorMessage(error);
  }
  notify("clients", "client");
}

/** v1 installs (no Game folder) cannot be launched. */
export function useClient(client: InstalledClient) {
  if (client.legacyLayout) return;
  state.executablePath = client.executablePath;
  state.settings.executablePath = client.executablePath;
  state.launchError = null;
  if (isValidLocale(client.locale) && client.locale !== state.locale) {
    state.locale = client.locale;
    state.settings.locale = client.locale;
    void refreshAssets();
    refreshChampionIndex();
  }
  void persistSettings();
  setStatus(`Using client ${client.resolvedVersion} (${client.locale})`);
  notify("client");
}

function updateDefaultDestination() {
  if (state.destinationEdited) return;
  const root = state.settings.clientsRoot;
  const patch = state.replay?.patch;
  state.destination = root && patch ? joinPath(root, `${patch}-${state.locale}`) : "";
}

function resetInstall() {
  state.install = idleInstall();
}

/** Drops a reviewed plan or a shown error once its inputs change. */
function invalidatePlan() {
  const phase = state.install.phase;
  if (phase === "review" || phase === "repairReview" || phase === "failed" || phase === "done") {
    resetInstall();
  }
}

function fail(op: InstallOp, error: unknown) {
  state.install.phase = "failed";
  state.install.op = op;
  state.install.error = toCommandError(error);
  state.install.cancelling = false;
}

export function installRequest(): ClientInstallRequest | null {
  const replay = state.replay;
  const destination = state.destination.trim();
  if (!replay?.patch || !destination || !isValidLocale(state.locale) || !state.region) return null;
  return {
    patch: replay.patch,
    gameVersion: replay.gameVersion,
    region: state.region,
    locale: state.locale,
    destinationPath: destination,
  };
}

export async function reviewInstallPlan() {
  const request = installRequest();
  if (!request || isInstallBusy(state.install.phase)) return;
  state.install = { ...idleInstall(), phase: "planning", op: "plan", request };
  notify("client");
  try {
    const plan = await api.planClientInstall(request);
    if (state.install.phase !== "planning") return;
    state.install.plan = plan;
    state.install.phase = "review";
  } catch (error) {
    if (state.install.phase !== "planning") return;
    fail("plan", error);
  }
  notify("client");
}

/** Discards a reviewed plan or a finished result. */
export function cancelInstallPlan() {
  if (isInstallBusy(state.install.phase)) return;
  resetInstall();
  notify("client");
}

export async function confirmInstall() {
  const plan = state.install.plan;
  if (!plan || state.install.phase !== "review") return;
  state.install.phase = "installing";
  state.install.op = "install";
  state.install.progress = null;
  notify("client");
  try {
    const result = await api.installClient(plan.planId);
    finish("install", result);
  } catch (error) {
    fail("install", error);
  }
  notify("client");
}

export async function reviewRepair() {
  const destinationPath = state.destination.trim();
  if (!destinationPath || !isValidLocale(state.locale) || isInstallBusy(state.install.phase)) return;
  state.install = { ...idleInstall(), phase: "repairPlanning", op: "repairPlan" };
  notify("client");
  try {
    const plan = await api.planClientRepair({ destinationPath, locale: state.locale });
    if (state.install.phase !== "repairPlanning") return;
    state.install.repairPlan = plan;
    state.install.phase = "repairReview";
  } catch (error) {
    if (state.install.phase !== "repairPlanning") return;
    fail("repairPlan", error);
  }
  notify("client");
}

export function setReplaceWrongSize(value: boolean) {
  if (state.install.phase !== "repairReview") return;
  state.install.replaceWrongSize = value;
  notify("client");
}

export async function confirmRepair() {
  const plan = state.install.repairPlan;
  if (!plan || state.install.phase !== "repairReview") return;
  state.install.phase = "repairing";
  state.install.op = "repair";
  state.install.progress = null;
  notify("client");
  try {
    const result = await api.repairClient({
      planId: plan.planId,
      replaceWrongSize: state.install.replaceWrongSize,
    });
    finish("repair", result);
  } catch (error) {
    fail("repair", error);
  }
  notify("client");
}

function finish(op: InstallOp, result: NonNullable<typeof state.install.result>) {
  state.install.result = result;
  state.install.phase = "done";
  state.install.op = op;
  state.install.cancelling = false;
  state.executablePath = result.executablePath;
  state.settings.executablePath = result.executablePath;
  state.launchError = null;
  void persistSettings();
  setStatus(`Client ${result.resolvedVersion} ${op === "repair" ? "repaired" : "installed"}`);
  void refreshClients();
}

/** Asks the backend to stop the running install or repair. Completed files stay. */
export async function cancelInstall() {
  const phase = state.install.phase;
  if (phase !== "installing" && phase !== "repairing") return;
  state.install.cancelling = true;
  notify("client");
  try {
    await api.cancelClientInstall();
  } catch (error) {
    state.install.cancelling = false;
    setStatus(`Could not cancel: ${errorMessage(error)}`, true);
    notify("client");
  }
}

export async function playReplay() {
  const replay = state.replay;
  if (!replay || replay.status !== "parsed" || !state.executablePath || state.launching) return;
  if (!isValidLocale(state.locale)) return;
  state.launching = true;
  state.launchError = null;
  notify("client");
  try {
    await api.launchReplay({
      replayPath: replay.path,
      executablePath: state.executablePath,
      locale: state.locale,
    });
    setStatus(`Launched ${replay.fileName} with ${state.locale}`);
  } catch (error) {
    state.launchError = errorMessage(error);
    setStatus("Launch failed", true);
  } finally {
    state.launching = false;
    notify("client");
  }
}
