// Client section: locale -> known client or executable -> optional install -> play.
// Every client action here is a direct response to a button press.

import { isTauri, PREVIEW_UNAVAILABLE, type InstalledClient } from "../api";
import {
  cancelInstallPlan,
  changeClientsRoot,
  chooseDestination,
  chooseExecutable,
  confirmInstall,
  installRequest,
  playReplay,
  reviewInstallPlan,
  setDestination,
  setExecutablePath,
  setLocale,
  useClient,
  useSuggestedLocale,
} from "../actions";
import { h, replace, syncValue } from "../dom";
import { formatBytes, formatNumber, isValidLocale, stageLabel } from "../format";
import { state, subscribe } from "../state";

function playBlocker(): string | null {
  const replay = state.replay;
  if (!replay) return "Select a replay first.";
  if (replay.status !== "parsed") return "This replay could not be parsed.";
  if (!isValidLocale(state.locale)) return "Enter a valid locale.";
  if (!state.executablePath) return "Choose a client executable.";
  if (state.install.phase === "installing") return "Wait for the install to finish.";
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
  const destinationInput = h("input", {
    class: "input mono grow",
    type: "text",
    spellcheck: false,
    autocomplete: "off",
    placeholder: "Folder for the new client",
    id: "destination-input",
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
    step("Install", "destination-input",
      installLead,
      h("div", { class: "field-row" },
        destinationInput,
        h("button", { class: "button", type: "button", text: "Browse…", on: { click: () => void chooseDestination() } }),
        reviewButton,
      ),
      installBody,
    ),
    step("Play", null, h("div", { class: "field-row" }, playButton, playHint)),
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
    const installing = install.phase === "installing";

    lead.textContent = patch
      ? `Needs a League of Legends ${patch} client. Nothing is downloaded, installed, or launched until you press a button below.`
      : "The patch is unknown, so no matching client can be suggested. You can still point to an executable.";

    syncValue(localeInput, state.locale);
    localeInput.disabled = installing;
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
    exeInput.disabled = installing;
    replace(clientsRootLine,
      "Clients folder: ",
      root ? h("span", { class: "mono", text: root }) : h("span", { text: "not set" }),
      " ",
      h("button", { class: "link", type: "button", text: root ? "Change" : "Choose…", on: { click: () => void changeClientsRoot() } }),
    );

    // Install
    installLead.textContent = patch
      ? `Download client ${patch} (${isValidLocale(state.locale) ? state.locale : "locale?"}) from Riot's patch servers into this folder:`
      : "Install needs a known patch.";
    syncValue(destinationInput, state.destination);
    destinationInput.disabled = installing || !patch;
    const request = installRequest();
    reviewButton.disabled = !request || install.phase === "planning" || installing || install.phase === "review";
    reviewButton.textContent = install.phase === "planning" ? "Planning…" : "Review install plan";
    replace(installBody, ...installView());

    const blocker = playBlocker();
    playHint.textContent = blocker ?? `${state.executablePath} · ${state.locale}`;
    playHint.title = playHint.textContent;
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
  return h("li", { class: "client-row" },
    h("span", { class: "client-info" },
      h("span", { text: `${client.resolvedVersion} · ${client.region} · ${client.locale}` }),
      h("span", { class: "sub mono", text: client.executablePath, title: client.executablePath }),
    ),
    client.executableExists
      ? h("button", {
          class: "button",
          type: "button",
          text: inUse ? "In use" : "Use",
          disabled: inUse || state.install.phase === "installing",
          on: { click: () => useClient(client) },
        })
      : h("span", { class: "hint error-text", text: "Executable missing" }),
  );
}

function installView(): Node[] {
  const install = state.install;
  switch (install.phase) {
    case "idle":
    case "planning":
      return isTauri ? [] : [h("p", { class: "hint", text: `Install and launch: ${PREVIEW_UNAVAILABLE.toLowerCase()}.` })];
    case "failed":
      return [
        h("p", { class: "error-text", text: install.error ?? "Install failed." }),
      ];
    case "review": {
      const plan = install.plan!;
      return [
        h("div", { class: "plan" },
          h("dl", { class: "plan-grid" },
            ...pair("Version", plan.resolvedVersion),
            ...pair("Region", plan.region),
            ...pair("Locale", plan.locale),
            ...pair("Destination", plan.destinationPath, true),
            ...pair("Files", formatNumber(plan.fileCount)),
            ...pair("Download", formatBytes(plan.downloadBytes)),
            ...pair("On disk", formatBytes(plan.installBytes)),
          ),
          h("p", { class: "hint", text: "Existing files are never overwritten. The install stops if any planned file already exists." }),
          h("div", { class: "field-row" },
            h("button", { class: "button primary", type: "button", text: "Install", on: { click: () => void confirmInstall() } }),
            h("button", { class: "button", type: "button", text: "Cancel", on: { click: () => cancelInstallPlan() } }),
          ),
        ),
      ];
    }
    case "installing": {
      const progress = install.progress;
      const percent = progress?.percent ?? null;
      const bar = h("div", {
        class: "progress",
        attrs: {
          role: "progressbar",
          "aria-valuemin": "0",
          "aria-valuemax": "100",
          ...(percent !== null ? { "aria-valuenow": String(Math.round(percent)) } : {}),
        },
      }, h("div", { class: "progress-fill", attrs: { style: `width:${Math.max(0, Math.min(100, percent ?? 0))}%` } }));
      const bytes = progress
        ? `${formatBytes(progress.downloadedBytes)}${progress.totalBytes ? ` of ${formatBytes(progress.totalBytes)}` : ""}`
        : "";
      return [
        h("div", { class: "plan" },
          h("div", { class: "progress-head" },
            h("span", { text: progress ? stageLabel(progress.stage) : "Starting…" }),
            h("span", { class: "num", text: percent !== null ? `${percent.toFixed(1)}%` : "" }),
          ),
          bar,
          h("div", { class: "progress-head sub" },
            h("span", { class: "mono ellipsis", text: progress?.detail ?? "", title: progress?.detail ?? "" }),
            h("span", { class: "num", text: bytes }),
          ),
          h("p", { class: "hint", text: `Installing into ${install.plan?.destinationPath ?? state.destination}` }),
        ),
      ];
    }
    case "done": {
      const result = install.result!;
      return [
        h("div", { class: "plan" },
          h("p", { text: `Client ${result.resolvedVersion} installed (${result.locale}).` }),
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

function pair(label: string, value: string, mono = false) {
  return [
    h("dt", { text: label }),
    h("dd", { class: mono ? "mono" : "", text: value, title: value }),
  ];
}
