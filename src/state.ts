import type {
  ClientInstallPlan,
  ClientInstallResult,
  DownloadProgress,
  InstalledClient,
  ReplayInspection,
  ReplayLibrary,
  Settings,
} from "./api";
import type { Asset, DataDragonAssets } from "./datadragon";

export type InstallPhase = "idle" | "planning" | "review" | "installing" | "done" | "failed";

export type Topic =
  | "settings"
  | "library"
  | "selection"
  | "replay"
  | "assets"
  | "clients"
  | "client"
  | "status";

export const state = {
  settings: {
    replayFolder: null,
    clientsRoot: null,
    executablePath: null,
    locale: null,
  } as Settings,

  library: null as ReplayLibrary | null,
  libraryLoading: false,
  libraryError: null as string | null,
  libraryChampions: null as Map<string, Asset> | null,
  filter: "",

  selectedPath: null as string | null,
  replay: null as ReplayInspection | null,
  replayLoading: false,
  replayError: null as string | null,
  assets: null as DataDragonAssets | null,
  assetsError: null as string | null,

  /** Editable locale. Starts from settings or the replay's suggestion, never forced. */
  locale: "en_US",
  executablePath: "",
  destination: "",
  destinationEdited: false,

  clients: [] as InstalledClient[],
  clientsError: null as string | null,

  install: {
    phase: "idle" as InstallPhase,
    plan: null as ClientInstallPlan | null,
    progress: null as DownloadProgress | null,
    result: null as ClientInstallResult | null,
    error: null as string | null,
  },
  launching: false,

  status: { text: "", error: false },
};

export type AppState = typeof state;

const listeners = new Map<Topic, Set<() => void>>();

export function subscribe(topics: Topic[], listener: () => void) {
  for (const topic of topics) {
    let set = listeners.get(topic);
    if (!set) {
      set = new Set();
      listeners.set(topic, set);
    }
    set.add(listener);
  }
}

/** Calls each subscribed listener once, even if it listens to several of the topics. */
export function notify(...topics: Topic[]) {
  const pending = new Set<() => void>();
  for (const topic of topics) {
    for (const listener of listeners.get(topic) ?? []) pending.add(listener);
  }
  for (const listener of pending) listener();
}

export function setStatus(text: string, error = false) {
  state.status = { text, error };
  notify("status");
}
