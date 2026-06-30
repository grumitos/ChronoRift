import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type ReplayInspection = {
  path: string;
  fileName: string;
  format: "classicRofl" | "rofl2" | "unknown";
  status: "parsed" | "unsupported" | "invalid";
  gameVersion: string | null;
  patch: string | null;
  region: string | null;
  suggestedLocale: string | null;
  matchId: string | null;
  gameDurationMs: number | null;
  metadata: unknown;
  notes: string[];
};

type LaunchReplayRequest = {
  replayPath: string;
  executablePath: string;
  locale: string;
};

type DownloadProgress = {
  destinationPath: string;
  downloadedBytes: number;
  totalBytes: number | null;
  percent: number | null;
  done: boolean;
};

const state: { replay: ReplayInspection | null } = { replay: null };

const openReplayButton = must("#open-replay", HTMLButtonElement);
const chooseInstallFolderButton = must("#choose-install-folder", HTMLButtonElement);
const downloadClientButton = must("#download-client", HTMLButtonElement);
const playReplayButton = must("#play-replay", HTMLButtonElement);
const installFolderInput = must("#install-folder", HTMLInputElement);
const exePathInput = must("#exe-path", HTMLInputElement);
const localeInput = must("#locale", HTMLInputElement);
const fileName = must("#file-name", HTMLElement);
const statusPill = must("#status-pill", HTMLElement);
const patch = must("#patch", HTMLElement);
const version = must("#version", HTMLElement);
const matchId = must("#match-id", HTMLElement);
const duration = must("#duration", HTMLElement);
const region = must("#region", HTMLElement);
const suggestedLocale = must("#suggested-locale", HTMLElement);
const clientState = must("#client-state", HTMLElement);
const progressBar = must("#progress-bar", HTMLElement);
const progressText = must("#progress-text", HTMLElement);
const metadata = must("#metadata", HTMLElement);
const note = must("#note", HTMLElement);

openReplayButton.addEventListener("click", chooseReplay);
chooseInstallFolderButton.addEventListener("click", chooseInstallFolder);
downloadClientButton.addEventListener("click", downloadClient);
playReplayButton.addEventListener("click", playReplay);
exePathInput.addEventListener("input", updateActions);
installFolderInput.addEventListener("input", updateActions);

void listen<DownloadProgress>("download-progress", (event) => {
  renderProgress(event.payload);
});

async function chooseReplay() {
  setBusy("Opening");
  try {
    const selected = await open({
      multiple: false,
      filters: [{ name: "League replay", extensions: ["rofl"] }],
    });

    if (typeof selected !== "string") {
      setStatus("Idle", "muted");
      return;
    }

    await inspectReplay(selected);
  } catch (error) {
    showError(error);
  }
}

async function chooseInstallFolder() {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected === "string") {
    installFolderInput.value = selected;
    updateActions();
  }
}

async function inspectReplay(path: string) {
  setBusy("Reading");
  try {
    const replay = await invoke<ReplayInspection>("inspect_replay", { path });
    state.replay = replay;
    renderReplay(replay);
  } catch (error) {
    showError(error);
  }
}

async function downloadClient() {
  const replay = state.replay;
  const installFolder = installFolderInput.value.trim();
  if (!replay?.patch || !installFolder) return;

  clientState.textContent = `Installer ready for ${replay.patch}`;
  progressText.textContent =
    "Manifest resolver is the next slice; download job plumbing is wired.";
  progressBar.style.width = "0%";

  // ponytail: real progress command exists; Riot manifest resolution lands before this button is enabled.
  downloadClientButton.disabled = true;
}

async function playReplay() {
  if (!state.replay) return;

  const request: LaunchReplayRequest = {
    replayPath: state.replay.path,
    executablePath: exePathInput.value.trim(),
    locale: localeInput.value.trim() || "en_US",
  };

  setBusy("Launching");
  try {
    await invoke("launch_replay", { request });
    setStatus("Launched", "ok");
  } catch (error) {
    showError(error);
  }
}

function renderReplay(replay: ReplayInspection) {
  fileName.textContent = replay.fileName;
  patch.textContent = replay.patch ?? "-";
  version.textContent = replay.gameVersion ?? "-";
  matchId.textContent = replay.matchId ?? "-";
  duration.textContent = formatDuration(replay.gameDurationMs);
  region.textContent = replay.region ?? "-";
  suggestedLocale.textContent = replay.suggestedLocale ?? "-";
  metadata.textContent = JSON.stringify(replay.metadata, null, 2);
  note.textContent = replay.notes.join(" ");

  if (replay.suggestedLocale) {
    localeInput.value = replay.suggestedLocale;
  }

  if (replay.status === "parsed") {
    setStatus("Parsed", "ok");
    clientState.textContent = replay.patch
      ? `Needs League client ${replay.patch}`
      : "Patch unknown";
    progressText.textContent =
      "No download starts automatically. Choose a folder, review locale, then install or point to an existing executable.";
  } else if (replay.status === "unsupported") {
    setStatus("Unsupported", "warn");
    clientState.textContent = "Unsupported replay format";
    progressText.textContent = "ROFL2 parser is intentionally not guessed yet.";
  } else {
    setStatus("Invalid", "bad");
    clientState.textContent = "Invalid replay";
    progressText.textContent = "ChronoRift could not read this file.";
  }

  updateActions();
}

function updateActions() {
  const replayReady = state.replay?.status === "parsed";
  playReplayButton.disabled = !replayReady || !exePathInput.value.trim();
  // Disabled until the Riot manifest resolver is implemented. Keeping it visible avoids hidden automation.
  downloadClientButton.disabled = true;
}

function renderProgress(progress: DownloadProgress) {
  const percent = progress.percent ?? 0;
  progressBar.style.width = `${Math.max(0, Math.min(percent, 100))}%`;
  progressText.textContent = progress.done
    ? `Downloaded ${progress.destinationPath}`
    : `${formatBytes(progress.downloadedBytes)} / ${
        progress.totalBytes ? formatBytes(progress.totalBytes) : "unknown"
      }`;
}

function formatDuration(ms: number | null) {
  if (!ms) return "-";
  const seconds = Math.round(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}

function setBusy(label: string) {
  setStatus(label, "muted");
}

function setStatus(label: string, tone: "muted" | "ok" | "warn" | "bad") {
  statusPill.textContent = label;
  statusPill.className = `pill ${tone}`;
}

function showError(error: unknown) {
  note.textContent = String(error);
  setStatus("Error", "bad");
}

function must<T extends Element>(selector: string, constructor: new () => T) {
  const element = document.querySelector(selector);
  if (!(element instanceof constructor)) {
    throw new Error(`Missing element: ${selector}`);
  }
  return element;
}
