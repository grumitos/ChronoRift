// User-triggered actions. Nothing in here downloads, installs, or launches a
// game client unless it is the direct handler of an explicit button.

import {
  api,
  errorMessage,
  onDownloadProgress,
  pickFile,
  pickFolder,
  type ClientInstallRequest,
  type InstalledClient,
} from "./api";
import { loadAssets, loadChampionIndex } from "./datadragon";
import { isValidLocale, joinPath } from "./format";
import { notify, setStatus, state } from "./state";

export async function init() {
  void onDownloadProgress((progress) => {
    if (state.install.phase !== "installing") return;
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

  if (state.settings.clientsRoot) void refreshClients();
  if (state.settings.replayFolder) await scan();
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
    if (library.replays.some((replay) => replay.blueChampions.length || replay.redChampions.length)) {
      void loadChampionIndex()
        .then((index) => {
          state.libraryChampions = index;
          notify("library");
        })
        .catch(() => {});
    }
  } catch (error) {
    state.library = null;
    state.libraryError = errorMessage(error);
  } finally {
    state.libraryLoading = false;
    notify("library");
  }
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
  if (state.install.phase !== "installing") resetInstall();
  notify("selection", "replay", "assets", "client");

  try {
    const replay = await api.inspectReplay(path);
    if (state.selectedPath !== path) return;
    state.replay = replay;
    if (!state.settings.locale && replay.suggestedLocale && isValidLocale(replay.suggestedLocale)) {
      // Default only: the field stays visible and editable.
      state.locale = replay.suggestedLocale;
    }
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
    state.assetsError = `Static data unavailable: ${errorMessage(error)}`;
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
  }
  notify("client");
}

export function useSuggestedLocale() {
  const suggested = state.replay?.suggestedLocale;
  if (suggested) setLocale(suggested);
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

export function useClient(client: InstalledClient) {
  state.executablePath = client.executablePath;
  state.settings.executablePath = client.executablePath;
  if (isValidLocale(client.locale) && client.locale !== state.locale) {
    state.locale = client.locale;
    state.settings.locale = client.locale;
    void refreshAssets();
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
  state.install = { phase: "idle", plan: null, progress: null, result: null, error: null };
}

function invalidatePlan() {
  if (state.install.phase === "review" || state.install.phase === "failed") resetInstall();
}

export function installRequest(): ClientInstallRequest | null {
  const replay = state.replay;
  const destination = state.destination.trim();
  if (!replay?.patch || !destination || !isValidLocale(state.locale)) return null;
  return {
    patch: replay.patch,
    gameVersion: replay.gameVersion,
    region: replay.region,
    locale: state.locale,
    destinationPath: destination,
  };
}

export async function reviewInstallPlan() {
  const request = installRequest();
  if (!request || state.install.phase === "installing") return;
  state.install = { phase: "planning", plan: null, progress: null, result: null, error: null };
  notify("client");
  try {
    const plan = await api.planClientInstall(request);
    if (state.install.phase !== "planning") return;
    state.install.plan = plan;
    state.install.phase = "review";
  } catch (error) {
    if (state.install.phase !== "planning") return;
    state.install.phase = "failed";
    state.install.error = errorMessage(error);
  }
  notify("client");
}

export function cancelInstallPlan() {
  if (state.install.phase === "installing") return;
  resetInstall();
  notify("client");
}

export async function confirmInstall() {
  const request = installRequest();
  if (!request || state.install.phase !== "review") return;
  state.install.phase = "installing";
  state.install.progress = null;
  notify("client");
  try {
    const result = await api.installClient(request);
    state.install.result = result;
    state.install.phase = "done";
    state.executablePath = result.executablePath;
    state.settings.executablePath = result.executablePath;
    void persistSettings();
    setStatus(`Client ${result.resolvedVersion} installed`);
    void refreshClients();
  } catch (error) {
    state.install.phase = "failed";
    state.install.error = errorMessage(error);
  }
  notify("client");
}

export async function playReplay() {
  const replay = state.replay;
  if (!replay || replay.status !== "parsed" || !state.executablePath || state.launching) return;
  if (!isValidLocale(state.locale)) return;
  state.launching = true;
  notify("client");
  try {
    await api.launchReplay({
      replayPath: replay.path,
      executablePath: state.executablePath,
      locale: state.locale,
    });
    setStatus(`Launched ${replay.fileName} with ${state.locale}`);
  } catch (error) {
    setStatus(`Launch failed: ${errorMessage(error)}`, true);
  } finally {
    state.launching = false;
    notify("client");
  }
}
