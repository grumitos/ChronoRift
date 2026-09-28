// Client section: locale -> known client or executable -> optional install -> play.
// Every client action here is a direct response to a button press.

import {
  isTauri,
  PREVIEW_UNAVAILABLE,
  REGIONS,
  type ClientInstallPlan,
  type ClientRepairPlan,
  type ClientSource,
  type CommandError,
  type InstalledClient,
} from "../api";
import {
  cancelInstall,
  cancelInstallPlan,
  changeClientsRoot,
  chooseDestination,
  chooseExecutable,
  confirmInstall,
  confirmRepair,
  installRequest,
  playReplay,
  reviewInstallPlan,
  reviewRepair,
  setDestination,
  setExecutablePath,
  setLocale,
  setRegion,
  setReplaceWrongSize,
  useClient,
  useSuggestedLocale,
} from "../actions";
import { h, replace, syncValue } from "../dom";
import { formatBytes, formatNumber, isValidLocale, stageLabel } from "../format";
import { isInstallBusy, state, subscribe } from "../state";

function playBlocker(): string | null {
  const replay = state.replay;
  if (!replay) return "Select a replay first.";
  if (replay.status !== "parsed") return "This replay could not be parsed.";
  if (!isValidLocale(state.locale)) return "Enter a valid locale.";
  if (!state.executablePath) return "Choose a client executable.";
  if (isInstallBusy(state.install.phase)) return "Wait for the install to finish.";
  return null;
}

export function createPlayButton(): HTMLButtonElement {
  const button = h("button", {
    class: "button primary",
    type: "button",
    text: "Play replay",
    on: { click: () => void playReplay() },
  });
  const update = () => {
    const blocker = playBlocker();
    button.disabled = blocker !== null || state.launching;
    button.title = blocker ?? `Launch with ${state.executablePath} (${state.locale})`;
    button.textContent = state.launching ? "Launching…" : "Play replay";
  };
  subscribe(["replay", "client", "settings"], update);
  update();
  return button;
}

export function mountClient(root: HTMLElement) {
  const lead = h("p", { class: "section-lead" });

  // Locale
  const localeInput = h("input", {
    class: "input input-locale mono",
    type: "text",
    maxLength: 5,
    spellcheck: false,
    autocomplete: "off",
    id: "locale-input",
    on: { change: () => setLocale(localeInput.value), input: () => updateLocaleHint() },
  });
  const localeHint = h("span", { class: "hint" });
  const localeError = h("p", { class: "field-error error-text", text: "Use the form xx_YY, for example en_US." });

  // Executable
  const knownClients = h("div", { class: "known-clients" });
  const exeInput = h("input", {
    class: "input mono grow",
    type: "text",
    spellcheck: false,
    autocomplete: "off",
    placeholder: "…\\Game\\League of Legends.exe",
    id: "exe-input",
    on: {
      input: () => setExecutablePath(exeInput.value, false),
      change: () => setExecutablePath(exeInput.value),
    },
  });
  const clientsRootLine = h("p", { class: "hint" });

  // Install
  const regionSelect = h("select", {
    class: "input input-server",
    id: "server-select",
    attrs: { "aria-label": "Server" },
    on: { change: () => setRegion(regionSelect.value) },
  },
    h("option", { value: "", text: "Server…" }),
    ...REGIONS.map((region) => h("option", { value: region, text: region })),
  );
  const regionHint = h("span", { class: "hint" });
  const destinationInput = h("input", {
    class: "input mono grow",
    type: "text",
    spellcheck: false,
    autocomplete: "off",
    placeholder: "Folder for the new client",
    id: "destination-input",
    attrs: { "aria-label": "Install destination" },
    on: { input: () => setDestination(destinationInput.value) },
  });
  const reviewButton = h("button", {
    class: "button",
    type: "button",
    text: "Review install plan",
    on: { click: () => void reviewInstallPlan() },
  });
  const installLead = h("p", { class: "hint" });
  const installBody = h("div", { class: "install-body" });

  // Play
  const playButton = createPlayButton();
  const playHint = h("span", { class: "hint" });
  const launchError = h("p", { class: "error-text launch-error" });

  root.append(
    h("h2", { class: "section-title", text: "Watch replay" }),
    lead,
    step("Locale", "locale-input",
      h("div", { class: "field-row" }, localeInput, localeHint),
      localeError,
    ),
    step("Client", "exe-input",
      knownClients,
      h("div", { class: "field-row" },
        exeInput,
        h("button", { class: "button", type: "button", text: "Browse…", on: { click: () => void chooseExecutable() } }),
      ),
      clientsRootLine,
    ),
    step("Install", "server-select",
      installLead,
      h("div", { class: "field-row" }, regionSelect, regionHint),
      h("div", { class: "field-row" },
        destinationInput,
        h("button", { class: "button", type: "button", text: "Browse…", on: { click: () => void chooseDestination() } }),
        reviewButton,
      ),
      installBody,
    ),
    step("Play", null, h("div", { class: "field-row" }, playButton, playHint), launchError),
  );

  function updateLocaleHint() {
    const replay = state.replay;
    const suggested = replay?.suggestedLocale;
    const typed = localeInput.value.trim();
    localeError.hidden = typed === "" || isValidLocale(typed);
    localeInput.setAttribute("aria-invalid", String(!localeError.hidden));
    if (suggested && suggested !== typed) {
      replace(localeHint,
        `Suggested for ${replay?.region ?? "this server"}: `,
        h("span", { class: "mono", text: suggested }),
        " ",
        h("button", { class: "link", type: "button", text: "Use", on: { click: () => useSuggestedLocale() } }),
      );
    } else if (suggested) {
      replace(localeHint, `Suggested for ${replay?.region ?? "this server"}`);
    } else {
      replace(localeHint, "Riot locale, for example en_US or ko_KR");
    }
  }

  const render = () => {
    const replay = state.replay;
    if (!replay) return;
    const patch = replay.patch;
    const install = state.install;
    const busy = isInstallBusy(install.phase);

    lead.textContent = patch
      ? `Needs a League of Legends ${patch} client. Nothing is downloaded, installed, or launched until you press a button below.`
      : "The patch is unknown, so no matching client can be suggested. You can still point to an executable.";

    syncValue(localeInput, state.locale);
    localeInput.disabled = busy;
    updateLocaleHint();

    // Known clients
    const matching = patch ? state.clients.filter((client) => client.patch === patch) : [];
    const root = state.settings.clientsRoot;
    if (!root) {
      replace(knownClients);
    } else if (state.clientsError) {
      replace(knownClients, h("p", { class: "hint error-text", text: `Could not read clients folder: ${state.clientsError}` }));
    } else if (matching.length === 0) {
      const others = state.clients.length;
      replace(knownClients, h("p", { class: "hint", text:
        `No installed ${patch ?? ""} client found${others ? ` (${others} other patch${others === 1 ? "" : "es"} installed)` : ""}.` }));
    } else {
      replace(knownClients, h("ul", { class: "client-list" }, ...matching.map(clientRow)));
    }

    syncValue(exeInput, state.executablePath);
    exeInput.disabled = busy;
    replace(clientsRootLine,
      "Clients folder: ",
      root ? h("span", { class: "mono", text: root }) : h("span", { text: "not set" }),
      " ",
      h("button", { class: "link", type: "button", text: root ? "Change" : "Choose…", on: { click: () => void changeClientsRoot() } }),
    );

    // Install
    installLead.textContent = patch
      ? `Download client ${patch} (${isValidLocale(state.locale) ? state.locale : "locale?"}) from Riot into this folder. Game files go in its Game folder.`
      : "Install needs a known patch.";
    if (document.activeElement !== regionSelect) regionSelect.value = state.region;
    regionSelect.disabled = busy || !patch;
    regionSelect.setAttribute("aria-invalid", String(install.error?.code === "invalidRegion"));
    const replayRegion = replay.region?.trim().toUpperCase() || null;
    if (!state.region) {
      regionHint.textContent = replayRegion
        ? `Replay server ${replayRegion} is not a known server. Choose one to install.`
        : "Replay server unknown. Choose one to install.";
    } else if (state.region === replayRegion) {
      regionHint.textContent = "Server from the replay";
    } else {
      regionHint.textContent = replayRegion ? `Replay was played on ${replayRegion}` : "Replay server unknown";
    }
    syncValue(destinationInput, state.destination);
    destinationInput.disabled = busy || !patch;
    const request = installRequest();
    reviewButton.disabled = !request || busy || install.phase === "review" || install.phase === "repairReview";
    reviewButton.textContent = install.phase === "planning" ? "Planning…" : "Review install plan";
    reviewButton.title = !patch
      ? "Install needs a known patch."
      : !state.region
        ? "Choose a server first."
        : !isValidLocale(state.locale)
          ? "Enter a valid locale."
          : !state.destination.trim()
            ? "Choose a destination folder."
            : "";
    replace(installBody, ...installView());

    const blocker = playBlocker();
    playHint.textContent = blocker ?? `${state.executablePath} · ${state.locale}`;
    playHint.title = playHint.textContent;
    launchError.textContent = state.launchError ?? "";
    launchError.hidden = !state.launchError;
  };

  subscribe(["replay", "client", "clients", "settings"], render);
  render();
}

function step(label: string, forId: string | null, ...body: Node[]) {
  return h("div", { class: "step" },
    forId
      ? h("label", { class: "step-label", htmlFor: forId, text: label })
      : h("span", { class: "step-label", text: label }),
    h("div", { class: "step-body" }, ...body),
  );
}

function clientRow(client: InstalledClient) {
  const inUse = client.executablePath === state.executablePath;
  const gameLayout = !client.legacyLayout;
  let action: Node;
  if (!client.executableExists) {
    action = h("span", { class: "hint error-text", text: "Executable missing" });
  } else {
    action = h("span", { class: "client-action" },
      gameLayout ? null : h("span", { class: "hint", text: "Old layout — reinstall to play" }),
      h("button", {
        class: "button",
        type: "button",
        text: inUse ? "In use" : "Use",
        disabled: !gameLayout || inUse || isInstallBusy(state.install.phase),
        title: gameLayout ? "" : "Installed before the Game folder layout; it cannot be launched.",
        on: { click: () => useClient(client) },
      }),
    );
  }
  return h("li", { class: "client-row" },
    h("span", { class: "client-info" },
      h("span", { text: `${client.resolvedVersion} · ${client.region} · ${client.locale}` }),
      h("span", { class: "sub mono", text: client.executablePath, title: client.executablePath }),
    ),
    action,
  );
}

function installView(): Node[] {
  const install = state.install;
  switch (install.phase) {
    case "idle":
    case "planning":
      return isTauri ? [] : [h("p", { class: "hint", text: `Install and launch: ${PREVIEW_UNAVAILABLE.toLowerCase()}.` })];
    case "repairPlanning":
      return [h("p", { class: "hint", text: `Checking the files in ${state.destination.trim()}…` })];
    case "failed":
      return [errorView(install.error ?? { code: "unknown", message: "Install failed." })];
    case "review":
      return [planView(install.plan!)];
    case "repairReview":
      return [repairView(install.repairPlan!)];
    case "installing":
    case "repairing":
      return [progressView()];
    case "done": {
      const result = install.result!;
      const repaired = install.op === "repair";
      return [
        h("div", { class: "plan" },
          h("p", { text: `Client ${result.resolvedVersion} ${repaired ? "repaired" : "installed"} (${result.locale}).` }),
          result.exactBuild ? null : h("p", { class: "notice", text: "Not the exact build of this replay. Playback may fail." }),
          h("dl", { class: "plan-grid" },
            ...pair("Executable", result.executablePath, true),
            ...pair("Files", formatNumber(result.fileCount)),
            ...pair("Downloaded", formatBytes(result.downloadBytes)),
            ...pair("On disk", formatBytes(result.installBytes)),
          ),
        ),
      ];
    }
  }
}

function sourceLabel(source: ClientSource, region: string, manifestRegion?: string): string {
  if (source === "sieve") return "Riot live (current patch)";
  return manifestRegion && manifestRegion !== region ? `Riot archive · ${manifestRegion} manifest` : "Riot archive";
}

function localeLabel(locale: string, assets: string): string {
  return assets && assets !== locale ? `${locale} · uses ${assets} files` : locale;
}

function inexactNotice(requested: string | null, resolved: string): HTMLElement {
  const text = requested
    ? `Not the exact build of this replay (replay ${requested}, closest available ${resolved}). Playback may fail.`
    : `The replay's exact build is unknown (closest available ${resolved}). Playback may fail.`;
  return h("p", { class: "notice", attrs: { role: "note" }, text });
}

function expiresLabel(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
}

function planView(plan: ClientInstallPlan): HTMLElement {
  return h("div", { class: "plan" },
    plan.exactBuild ? null : inexactNotice(plan.requestedVersion, plan.resolvedVersion),
    h("dl", { class: "plan-grid" },
      ...pair("Version", plan.resolvedVersion),
      ...pair("Source", sourceLabel(plan.source, plan.region, plan.manifestRegion)),
      ...pair("Server", plan.region),
      ...pair("Locale", localeLabel(plan.locale, plan.localeAssets)),
      ...pair("Destination", plan.destinationPath, true),
      ...pair("Game folder", plan.gamePath, true),
      ...pair("Files", formatNumber(plan.fileCount)),
      ...pair("Download", formatBytes(plan.downloadBytes)),
      ...pair("On disk", formatBytes(plan.installBytes)),
    ),
    h("p", { class: "hint", text:
      `${formatNumber(plan.requestCount)} download requests. Existing files are never overwritten. Plan valid until ${expiresLabel(plan.expiresAtMs)}.` }),
    h("div", { class: "field-row" },
      h("button", { class: "button primary", type: "button", text: "Install", on: { click: () => void confirmInstall() } }),
      h("button", { class: "button", type: "button", text: "Cancel", on: { click: () => cancelInstallPlan() } }),
    ),
  );
}

function repairView(plan: ClientRepairPlan): HTMLElement {
  const replaceWrong = state.install.replaceWrongSize;
  const download = plan.downloadBytes + (replaceWrong ? plan.replaceDownloadBytes : 0);
  const writes = plan.installBytes + (replaceWrong ? plan.replaceInstallBytes : 0);
  const counts = [
    `${formatNumber(plan.okFiles)} ok`,
    `${formatNumber(plan.missingFiles)} missing`,
    `${formatNumber(plan.wrongSizeFiles)} wrong size`,
  ].join(" · ");
  const nothingToDo = plan.missingFiles === 0 && plan.wrongSizeFiles === 0 && plan.staleTempFiles === 0;
  const checkbox = h("input", {
    type: "checkbox",
    checked: replaceWrong,
    disabled: plan.wrongSizeFiles === 0,
    on: { change: () => setReplaceWrongSize(checkbox.checked) },
  });
  return h("div", { class: "plan" },
    h("p", { text: `Repair client ${plan.resolvedVersion} (${plan.locale}).` }),
    plan.exactBuild
      ? null
      : h("p", {
          class: "notice",
          attrs: { role: "note" },
          text: plan.requestedVersion
            ? `This install is not the exact build of its replay (replay ${plan.requestedVersion}, closest ${plan.resolvedVersion}). Playback may fail.`
            : "This install is not the exact build of its replay. Playback may fail.",
        }),
    plan.stateStatus === "inProgress"
      ? h("p", { class: "hint", text: "The last install or repair in this folder did not finish." })
      : null,
    h("dl", { class: "plan-grid" },
      ...pair("Source", sourceLabel(plan.source, plan.region)),
      ...pair("Locale", localeLabel(plan.locale, plan.localeAssets)),
      ...pair("Game folder", plan.gamePath, true),
      ...pair("Files", `${counts} of ${formatNumber(plan.totalFiles)}`),
      ...(plan.staleTempFiles > 0
        ? pair("Temp files", `${formatNumber(plan.staleTempFiles)} left over, will be deleted`)
        : []),
      ...pair("Download", formatBytes(download)),
      ...pair("Writes", formatBytes(writes)),
    ),
    pathList("Missing files", plan.missingPaths, plan.missingFiles),
    pathList("Files with wrong size", plan.wrongSizePaths, plan.wrongSizeFiles),
    h("label", { class: "check" },
      checkbox,
      h("span", { text: "Replace files with wrong size" }),
      plan.wrongSizeFiles > 0
        ? h("span", { class: "hint", text: `+${formatBytes(plan.replaceDownloadBytes)} download` })
        : null,
    ),
    h("p", { class: "hint", text: nothingToDo
      ? "All files are present. Repair only records the install as complete."
      : "Only missing files are downloaded. Files with the wrong size stay as they are unless you replace them." }),
    h("div", { class: "field-row" },
      h("button", { class: "button primary", type: "button", text: "Repair", on: { click: () => void confirmRepair() } }),
      h("button", { class: "button", type: "button", text: "Cancel", on: { click: () => cancelInstallPlan() } }),
    ),
  );
}

function pathList(label: string, paths: string[], total: number): HTMLElement | null {
  if (total === 0) return null;
  const shown = paths.length < total ? ` (first ${formatNumber(paths.length)} of ${formatNumber(total)})` : "";
  return h("details", { class: "path-details" },
    h("summary", { text: `${label}${shown}` }),
    h("ul", { class: "path-list mono" }, ...paths.map((path) => h("li", { text: path, title: path }))),
  );
}

function progressView(): HTMLElement {
  const install = state.install;
  const progress = install.progress;
  const repairing = install.phase === "repairing";
  const percent = progress?.percent ?? null;
  const fill = h("div", { class: "progress-fill" });
  fill.style.width = `${Math.max(0, Math.min(100, percent ?? 0))}%`;
  const bar = h("div", {
    class: "progress",
    attrs: {
      role: "progressbar",
      "aria-valuemin": "0",
      "aria-valuemax": "100",
      ...(percent !== null ? { "aria-valuenow": String(Math.round(percent)) } : {}),
    },
  }, fill);
  const bytes = progress
    ? `${formatBytes(progress.downloadedBytes)}${progress.totalBytes ? ` of ${formatBytes(progress.totalBytes)}` : ""}`
    : "";
  const files = progress && progress.filesTotal > 0
    ? `${formatNumber(progress.filesDone)} of ${formatNumber(progress.filesTotal)} files`
    : "";
  const destination = install.plan?.destinationPath ?? install.repairPlan?.destinationPath ?? state.destination;
  return h("div", { class: "plan" },
    h("div", { class: "progress-head" },
      h("span", { text: progress ? stageLabel(progress.stage) : "Starting…" }),
      h("span", { class: "num", text: percent !== null ? `${percent.toFixed(1)}%` : "" }),
    ),
    bar,
    h("div", { class: "progress-head sub" },
      h("span", { class: "num", text: files }),
      h("span", { class: "num", text: bytes }),
    ),
    progress?.detail
      ? h("p", { class: "sub mono ellipsis", text: progress.detail, title: progress.detail })
      : null,
    h("div", { class: "field-row" },
      h("span", { class: "hint grow", text: `${repairing ? "Repairing" : "Installing into"} ${destination}`, title: destination }),
      h("button", {
        class: "button",
        type: "button",
        text: install.cancelling ? "Cancelling…" : "Cancel",
        disabled: install.cancelling,
        on: { click: () => void cancelInstall() },
      }),
    ),
  );
}

function reviewRepairButton(): HTMLButtonElement {
  return h("button", { class: "button", type: "button", text: "Review repair", on: { click: () => void reviewRepair() } });
}

function errorView(error: CommandError): HTMLElement {
  const op = state.install.op;
  const pathLine = (path: string | undefined) =>
    path && !error.message.includes(path) ? h("p", { class: "hint mono", text: path, title: path }) : null;
  switch (error.code) {
    case "localeUnavailable": {
      const locale = state.install.request?.locale ?? state.locale;
      const available = error.availableLocales ?? [];
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: `Locale ${locale} is not offered by this client.` }),
        available.length
          ? h("p", { class: "hint", text: "Choose one of the available locales, then review the plan again:" })
          : null,
        available.length
          ? h("div", { class: "locale-list" },
              ...available.map((item) =>
                h("button", { class: "link mono", type: "button", text: item, on: { click: () => setLocale(item) } })))
          : null,
      );
    }
    case "wouldOverwrite":
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: error.message }),
        pathLine(error.path),
        h("div", { class: "field-row" },
          reviewRepairButton(),
          h("span", { class: "hint", text: "Checks the files already in this folder. Nothing changes until you press Repair." }),
        ),
      );
    case "cancelled":
      if (op === "install" || op === "repair") {
        return h("div", { class: "stack" },
          h("p", { text: "Install cancelled. Review repair to resume." }),
          h("div", { class: "field-row" }, reviewRepairButton()),
        );
      }
      return h("div", { class: "stack" }, h("p", { text: "Planning cancelled." }));
    case "unknownPlan":
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: "This plan expired. Review it again before continuing." }),
        h("div", { class: "field-row" },
          op === "repair"
            ? reviewRepairButton()
            : h("button", { class: "button", type: "button", text: "Review install plan", on: { click: () => void reviewInstallPlan() } }),
        ),
      );
    case "notChronoRiftInstall":
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: "This folder was not installed by ChronoRift, so it cannot be repaired." }),
        pathLine(error.path),
        h("p", { class: "hint", text: "Choose an empty folder to install into." }),
      );
    case "localeMismatch":
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: error.message }),
        h("p", { class: "hint", text: "Set the locale to the installed one to repair it, or choose another folder." }),
      );
    case "busy":
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: "Another install is running. Wait for it to finish, then try again." }),
      );
    default:
      return h("div", { class: "stack" },
        h("p", { class: "error-text", text: error.message }),
        pathLine(error.path),
      );
  }
}

function pair(label: string, value: string, mono = false) {
  return [
    h("dt", { text: label }),
    h("dd", { class: mono ? "mono" : "", text: value, title: value }),
  ];
}
