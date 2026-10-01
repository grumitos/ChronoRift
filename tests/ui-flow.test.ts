// Flujo completo de la UI real (index.html + src/main.ts) sobre happy-dom, con el IPC de Tauri
// simulado por @tauri-apps/api/mocks y los datos estáticos servidos por un fetch local.
// Los datos son sintéticos: no se usan Riot IDs reales. Los errores son los textos del backend.
import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { afterAll, beforeAll, describe, expect, setSystemTime, test } from "bun:test";

const REPLAY_DIR = "C:\\Replays";
const OTHER_DIR = "D:\\Otras";
const USB_DIR = "F:\\Repeticiones";
const REPLAY_A = `${REPLAY_DIR}\\LA2-1625354615.rofl`;
const REPLAY_B = `${REPLAY_DIR}\\LA2-1621247134.rofl`;
const CLIENTS_ROOT = "E:\\ChronoRift";
const BUILD_A = "16.18.817.5716";
const BUILD_B = "16.17.810.4348";
// Compilación más nueva del parche 16.17, instalada en lugar de la exacta de la repetición B.
const BUILD_B_NEWER = "16.17.811.2000";
const DESTINATION = `${CLIENTS_ROOT}\\${BUILD_A}\\Game`;
const EXECUTABLE = `${DESTINATION}\\League of Legends.exe`;
const DD = "https://ddragon.leagueoflegends.com";
const CD = "https://raw.communitydragon.org/16.18/plugins/rcp-be-lol-game-data/global";
// WebView2 (Windows) expone el esquema propio como http://lolasset.localhost: con el agente de
// usuario de Edge en Windows, la app pide DataDragon y CommunityDragon a la caché en disco así.
const EDGE_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
const CACHE = "http://lolasset.localhost";
const ORIGINS: Record<string, string> = { ddragon: DD, cdragon: "https://raw.communitydragon.org" };
const viaCache = (url: string) =>
  url.startsWith(`${DD}/`) ? `${CACHE}/ddragon${url.slice(DD.length)}` : `${CACHE}/cdragon${url.slice(ORIGINS.cdragon.length)}`;
const fromCache = (url: string) => {
  const match = url.match(/^http:\/\/lolasset\.localhost\/(ddragon|cdragon)(\/.*)$/);
  return match ? `${ORIGINS[match[1]]}${match[2]}` : null;
};
const GIB = 1024 ** 3;
const OWNER = "Yo#LAS";
const NETWORK_FAILURE = "Falló la descarga: tiempo de espera agotado (lol.dyn.riotcdn.net)";

const champions = ["Urgot", "MonkeyKing", "Veigar", "Caitlyn", "Pyke", "Sett", "Quinn", "Hwei", "Smolder", "Nautilus"];
const positions = ["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];

// Jugador 8 (rojo, BOT) es el dueño de la biblioteca: el único que se repite entre partidas.
function players(redWins: boolean, game: string) {
  return champions.map((champion, index) => {
    const [gameName, tagLine] = index === 8 ? ["Yo", "LAS"] : [`Jugador${game}${index}`, "LAS"];
    const red = index >= 5;
    return {
      name: `${gameName}#${tagLine}`,
      gameName,
      tagLine,
      champion,
      championImageUrl: `${DD}/cdn/16.18.1/img/champion/${champion}.png`,
      team: red ? "Red" : "Blue",
      position: positions[index % 5],
      win: red === redWins,
      level: 16 + (index % 3),
      kills: index + 1,
      deaths: 2,
      assists: 3,
      creepScore: 150 + index,
      goldEarned: 12_000 + index * 100,
      damageToChampions: 20_000 + index * 1_000,
      physicalDamageToChampions: 12_000 + index * 600,
      magicDamageToChampions: 7_000 + index * 300,
      trueDamageToChampions: 1_000 + index * 100,
      damageTaken: 25_000,
      damageSelfMitigated: 18_000,
      healShieldOnTeammates: index === 4 ? 9_000 : 0,
      timeDeadSeconds: 120 + index,
      visionScore: 30,
      turretKills: 1,
      dragonKills: index === 1 ? 2 : 0,
      baronKills: 0,
      heraldKills: 0,
      voidgrubKills: 0,
      items: [
        { slot: 0, id: "3508", imageUrl: `${DD}/cdn/16.18.1/img/item/3508.png` },
        // Misión de rol (ROLE_BOUND_ITEM): solo el dueño en la partida A; la B no tiene ninguna.
        ...(index === 8 && game === BUILD_A
          ? [{ slot: 7, id: "3020", imageUrl: `${DD}/cdn/16.18.1/img/item/3020.png` }]
          : []),
      ],
      summonerSpells: ["4", "14"],
      perks: ["8005"],
      keystone: "8005",
      primaryStyle: "8000",
      subStyle: "8400",
    };
  });
}

function teams(list: ReturnType<typeof players>) {
  return ["Blue", "Red"].map((team) => {
    const members = list.filter((player) => player.team === team);
    return {
      team,
      win: members.some((player) => player.win),
      kills: members.reduce((total, player) => total + player.kills, 0),
      goldEarned: members.reduce((total, player) => total + player.goldEarned, 0),
      damageToChampions: members.reduce((total, player) => total + player.damageToChampions, 0),
      towers: team === "Blue" ? 3 : 9,
      inhibitors: team === "Blue" ? 0 : 2,
      dragons: members.reduce((total, player) => total + player.dragonKills, 0),
      barons: 0,
      heralds: 0,
      voidgrubs: 0,
      atakhan: 0,
    };
  });
}

function inspection(path: string, version: string, patch: string, durationMs: number, redWins: boolean) {
  const list = players(redWins, version);
  return {
    path,
    fileName: path.slice(REPLAY_DIR.length + 1),
    format: "rofl2",
    status: "parsed",
    gameVersion: version,
    patch,
    region: "LA2",
    suggestedLocale: "es_MX",
    dataDragonVersion: `${patch}.1`,
    matchId: path.slice(REPLAY_DIR.length + 5, -5),
    gameDurationMs: durationMs,
    winningTeam: redWins ? "Red" : "Blue",
    blueKills: 15,
    redKills: 40,
    players: list,
    teams: teams(list),
    notes: [],
  };
}

const replays: Record<string, ReturnType<typeof inspection>> = {
  [REPLAY_A]: inspection(REPLAY_A, BUILD_A, "16.18", 2_194_553, true),
  [REPLAY_B]: inspection(REPLAY_B, BUILD_B, "16.17", 2_698_573, false),
};

const library = {
  folderPath: REPLAY_DIR,
  entries: [REPLAY_A, REPLAY_B].map((path, index) => {
    const replay = replays[path];
    return {
      path,
      fileName: replay.fileName,
      fileSize: 19_000_000,
      modifiedAtMs: 1_758_344_477_000 - index * 86_400_000,
      format: "rofl2",
      status: "parsed",
      gameVersion: replay.gameVersion,
      patch: replay.patch,
      region: "LA2",
      matchId: replay.matchId,
      gameDurationMs: replay.gameDurationMs,
      winningTeam: replay.winningTeam,
      blueKills: 15,
      redKills: 40,
      playerCount: 10,
      players: replay.players.map(({ name, champion, team, position, kills, deaths, assists, win }) => ({
        name, champion, team, position, kills, deaths, assists, win,
      })),
    };
  }),
  failures: [{ path: `${REPLAY_DIR}\\broken.rofl`, message: "Cabecera ROFL no válida: falta la firma RIOT clásica" }],
};

function plan(mode: string, locale: string) {
  return {
    mode,
    patch: "16.18",
    resolvedVersion: "16.18.8175716",
    gameBuild: BUILD_A,
    exactBuild: true,
    source: "manifestIndex",
    region: "LA2",
    locale,
    clientsRoot: CLIENTS_ROOT,
    destinationPath: DESTINATION,
    manifestUrl: "https://lol.secure.dyn.riotcdn.net/channels/public/releases/344E097FEB027691.manifest",
    fileCount: 4_056,
    affectedFileCount: mode === "resume" ? 293 : 416,
    reusedFileCount: mode === "resume" ? 123 : 0,
    repairedFileCount: 0,
    downloadBytes: (mode === "resume" ? 18 : 23.5) * GIB,
    installBytes: 29.3 * GIB,
    totalInstallBytes: 29.3 * GIB,
  };
}

// Los mismos datos en inglés, con otros nombres: al cambiar el idioma, los de la partida cambian.
const englishData: Record<string, unknown> = {
  [`${DD}/cdn/16.18.1/data/en_US/champion.json`]: {
    data: Object.fromEntries(champions.map((id) => [id, { id, name: id === "MonkeyKing" ? "Wukong" : id, image: { full: `${id}.png` } }])),
  },
  [`${DD}/cdn/16.18.1/data/en_US/item.json`]: {
    data: {
      "3508": { name: "Stormsurge", image: { full: "3508.png" } },
      "3020": { name: "Sorcerer's Shoes", image: { full: "3020.png" } },
    },
  },
  [`${CD}/en_us/v1/summoner-spells.json`]: [
    { id: 4, name: "Flash", description: "Teleports", iconPath: "/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_Flash.png" },
    { id: 14, name: "Ignite", description: "Burns", iconPath: "/lol-game-data/assets/DATA/Spells/Icons2D/SummonerIgnite.png" },
  ],
  [`${CD}/en_us/v1/perks.json`]: [
    { id: 8005, name: "Press the Attack", iconPath: "/lol-game-data/assets/v1/perk-images/Styles/Precision/PressTheAttack/PressTheAttack.png" },
  ],
  [`${CD}/en_us/v1/perkstyles.json`]: {
    styles: [{ id: 8400, name: "Resolve", iconPath: "/lol-game-data/assets/v1/perk-images/Styles/7204_Resolve.png" }],
  },
};

const staticData: Record<string, unknown> = {
  ...englishData,
  [`${DD}/api/versions.json`]: ["16.19.1", "16.18.1", "16.17.1"],
  [`${DD}/cdn/16.18.1/data/es_MX/champion.json`]: {
    data: Object.fromEntries(
      champions.map((id) => [id, { id, name: id === "MonkeyKing" ? "Wukong" : id, image: { full: `${id}.png` } }]),
    ),
  },
  [`${DD}/cdn/16.18.1/data/es_MX/item.json`]: {
    data: {
      "3508": { name: "Filo de la tormenta", image: { full: "3508.png" } },
      "3020": { name: "Botas del Hechicero", image: { full: "3020.png" } },
    },
  },
  [`${CD}/es_mx/v1/summoner-spells.json`]: [
    { id: 4, name: "Destello", description: "Teletransporta", iconPath: "/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_Flash.png" },
    { id: 14, name: "Ignición", description: "Quema", iconPath: "/lol-game-data/assets/DATA/Spells/Icons2D/SummonerIgnite.png" },
  ],
  [`${CD}/es_mx/v1/perks.json`]: [
    { id: 8005, name: "Ataque intensificado", iconPath: "/lol-game-data/assets/v1/perk-images/Styles/Precision/PressTheAttack/PressTheAttack.png" },
  ],
  [`${CD}/es_mx/v1/perkstyles.json`]: {
    styles: [{ id: 8400, name: "Valor", iconPath: "/lol-game-data/assets/v1/perk-images/Styles/7204_Resolve.png" }],
  },
};

type Call = { cmd: string; args: any };
const calls: Call[] = [];
const fetched: string[] = [];
const dialogAnswers: Array<string | null> = [];
// Primera vez: el idioma nunca se eligió y el del sistema es español.
let settings: { clientsRoot: string | null; assetPrefetch?: boolean; language: string | null; systemLanguage: string } = {
  clientsRoot: CLIENTS_ROOT,
  language: null,
  systemLanguage: "es",
};
let planFailure: string | null = null;
let launchFailure: string | null = null;
let revealFailure: string | null = null;
let folderFailure: string | null = null;
// Carpeta de clientes en una unidad desconectada: los comandos que la leen fallan como en Rust.
let unavailableRoot: string | null = null;
let vanguard = "stopped";
let pendingInstall: { resolve: (value: unknown) => void; reject: (reason: unknown) => void } | null = null;
// Detener retenido: el backend recibe la orden pero todavía no termina de cerrar la descarga.
let holdCancel = false;
let heldCancel: (() => void) | null = null;
// Lecturas de repeticiones retenidas hasta que la prueba las suelta.
let holdInspect = false;
const heldInspects: Array<() => void> = [];
// Estado en disco que devuelve el backend simulado para cada versión de repetición; `build` es otra
// compilación del mismo parche instalada en su lugar.
type Disk = { state: string; installedBytes: number; totalInstallBytes: number | null; build?: string };
const disk: Record<string, Disk> = {
  [BUILD_A]: { state: "missing", installedBytes: 0, totalInstallBytes: null },
  [BUILD_B]: { state: "missing", installedBytes: 0, totalInstallBytes: null },
};
const realFetch = globalThis.fetch;

function localStatus(version: string) {
  const current = disk[version];
  const build = current.build ?? version;
  const destinationPath = `${settings.clientsRoot}\\${build}\\Game`;
  return {
    gameBuild: build,
    exactBuild: build === version,
    destinationPath,
    state: current.state,
    executablePath: current.state === "ready" ? `${destinationPath}\\League of Legends.exe` : null,
    locale: current.state === "missing" ? null : "es_MX",
    installedBytes: current.installedBytes,
    totalInstallBytes: current.totalInstallBytes,
  };
}

function unavailable(root: string) {
  if (root === unavailableRoot) throw `La carpeta de clientes no está disponible: ${root}`;
}

// Backend simulado: responde como los comandos de Rust y registra cada llamada.
async function backend(cmd: string, args: any) {
  calls.push({ cmd, args });
  switch (cmd) {
    case "get_settings":
      return { ...settings };
    // Como `settings::update`: cambia un ajuste y conserva los demás.
    case "set_clients_root": {
      const path = String(args.path).trim();
      settings = { ...settings, clientsRoot: /^[A-Za-z]:\\?$/.test(path) ? `${path.slice(0, 2)}\\ChronoRift` : path };
      return { ...settings };
    }
    case "set_asset_prefetch":
      settings = { ...settings, assetPrefetch: args.enabled };
      return { ...settings };
    case "set_language":
      settings = { ...settings, language: args.language };
      return { ...settings };
    // Como la caché en disco: descarga las rutas `lolasset` del parche, avisa el avance y responde
    // con la cuenta. Aquí todas existen.
    case "prefetch_assets": {
      const total = args.paths.length;
      await emitEvent("asset-prefetch-progress", { done: total, total, ok: total, missing: 0, errors: 0 });
      return { done: total, total, ok: total, missing: 0, errors: 0, cancelled: false, offline: false };
    }
    case "cancel_asset_prefetch":
      return null;
    case "plugin:dialog|open":
      return dialogAnswers.shift() ?? null;
    case "plugin:opener|reveal_item_in_dir":
      if (revealFailure) throw revealFailure;
      return null;
    case "plugin:opener|open_url":
      return null;
    case "inspect_replay_folder":
      if (folderFailure) throw folderFailure;
      return args.path === REPLAY_DIR ? library : { folderPath: args.path, entries: [], failures: [] };
    case "inspect_replay":
      if (holdInspect) return new Promise((resolve) => heldInspects.push(() => resolve(replays[args.path])));
      return replays[args.path];
    case "client_statuses":
      unavailable(args.clientsRoot);
      return Object.fromEntries(args.versions.map((version: string) => [version, localStatus(version)]));
    case "list_clients":
      unavailable(args.clientsRoot);
      return Object.keys(disk).filter((version) => disk[version].state !== "missing").map(localStatus);
    case "plan_client_install":
      if (planFailure) throw planFailure;
      return plan(disk[BUILD_A].state === "partial" ? "resume" : "install", args.request.locale);
    case "install_client":
      return new Promise((resolve, reject) => {
        pendingInstall = { resolve, reject };
      });
    case "cancel_client_install": {
      const settle = () => {
        disk[BUILD_A] = { state: "partial", installedBytes: 6 * GIB, totalInstallBytes: 29.3 * GIB };
        pendingInstall?.reject("Descarga detenida");
      };
      if (holdCancel) heldCancel = settle;
      else settle();
      return null;
    }
    case "launch_replay":
      if (launchFailure) throw launchFailure;
      return null;
    case "vanguard_status":
      return vanguard;
    default:
      throw new Error(`Unexpected command ${cmd}`);
  }
}

function el<T extends Element = HTMLElement>(selector: string): T {
  const node = document.querySelector(selector);
  if (!node) throw new Error(`Missing ${selector}`);
  return node as unknown as T;
}

const text = (selector: string) => el(selector).textContent?.replace(/\s+/g, " ").trim() ?? "";
const called = (cmd: string) => calls.filter((call) => call.cmd === cmd);
const click = (selector: string) => el<HTMLButtonElement>(selector).click();
const disabled = (selector: string) => el<HTMLButtonElement>(selector).disabled;

// Con `performance.now()`: las pruebas de velocidad fijan la hora de `Date` con `setSystemTime`.
async function waitFor(predicate: () => boolean, label: string, timeoutMs = 3_000) {
  const start = performance.now();
  while (!predicate()) {
    if (performance.now() - start > timeoutMs) throw new Error(`Timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

function typeInto(selector: string, value: string, events: string[] = ["input"]) {
  const field = el<HTMLInputElement | HTMLSelectElement>(selector);
  field.value = value;
  for (const type of events) field.dispatchEvent(new Event(type, { bubbles: true }));
}

async function emitEvent(name: string, payload: unknown) {
  const { emit } = await import("@tauri-apps/api/event");
  await emit(name, payload);
}

async function emitProgress(stage: string, downloadedBytes: number, totalBytes: number | null, detail = "") {
  await emitEvent("download-progress", {
    destinationPath: DESTINATION,
    downloadedBytes,
    totalBytes,
    percent: totalBytes ? (downloadedBytes / totalBytes) * 100 : null,
    done: stage === "complete",
    stage,
    detail,
  });
}

/** Directivas de una CSP de Tauri, escrita como texto o como objeto. */
function cspDirectives(policy: string | Record<string, string | string[]>) {
  const pairs = typeof policy === "string"
    ? policy.split(";").map((part) => part.trim().split(/\s+/)).filter(([name]) => name).map(([name, ...sources]) => [name, sources])
    : Object.entries(policy).map(([name, sources]) => [name, Array.isArray(sources) ? sources : sources.split(/\s+/)]);
  return Object.fromEntries(pairs) as Record<string, string[]>;
}

beforeAll(async () => {
  GlobalRegistrator.register({
    url: "http://localhost:1420/",
    width: 1360,
    height: 840,
    settings: { navigator: { userAgent: EDGE_UA } },
  });
  const html = await Bun.file(new URL("../index.html", import.meta.url)).text();
  document.body.innerHTML = html
    .slice(html.indexOf("<body>") + "<body>".length, html.indexOf("</body>"))
    .replace(/<script\b[\s\S]*?<\/script>/g, "");
  globalThis.fetch = (async (input: string | URL | Request) => {
    const url = input instanceof Request ? input.url : String(input);
    fetched.push(url);
    // Solo responde lo que llega por la caché: una petición directa al CDN queda en 404.
    const upstream = fromCache(url);
    const body = upstream ? staticData[upstream] : undefined;
    return body === undefined
      ? new Response("not found", { status: 404 })
      : new Response(JSON.stringify(body), { status: 200 });
  }) as typeof fetch;

  const { mockIPC } = await import("@tauri-apps/api/mocks");
  mockIPC(backend, { shouldMockEvents: true });
  await import("../src/main.ts");
  await waitFor(() => el<HTMLInputElement>("#install-folder").value === CLIENTS_ROOT, "persisted clients folder");
});

afterAll(async () => {
  const { clearMocks } = await import("@tauri-apps/api/mocks");
  clearMocks();
  globalThis.fetch = realFetch;
  await GlobalRegistrator.unregister();
});

describe("library → match → client → launch", () => {
  test("the first run asks the language on the splash, switches live and can change it in Clientes", async () => {
    // Sin idioma elegido y sin carpeta recordada, la pantalla de inicio pregunta solo el idioma, con
    // el del sistema que dice el backend (español) marcado, aunque el visor diga inglés.
    expect(el("#splash").hidden).toBe(false);
    expect(el("#splash-ask").hidden).toBe(false);
    expect(el("#setup-language").hidden).toBe(false);
    expect(el("#setup-images").hidden).toBe(true);
    expect(el("#setup-continue").hidden).toBe(false);
    expect(el("#splash-ask").getAttribute("aria-labelledby")).toBe("setup-language-label");
    expect(el("#setup-languages [data-language='es']").getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.lang).toBe("es");
    expect(text("#setup-continue")).toBe("Continuar");
    expect(el("#library-view").inert).toBe(true);
    // Los botones de la ventana siguen fuera de lo inerte, con sus iconos desde el HTML.
    expect(el("#window-close").closest("[inert]")).toBeNull();
    expect(el("#window-close svg")).not.toBeNull();

    // Elegir inglés lo guarda y cambia todo en el acto, sin recargar: textos fijos y pintados.
    click("#setup-languages [data-language='en']");
    await waitFor(() => called("set_language").length === 1, "saved language");
    expect(called("set_language")[0].args).toEqual({ language: "en" });
    expect(document.documentElement.lang).toBe("en");
    expect(el("#setup-languages [data-language='en']").getAttribute("aria-pressed")).toBe("true");
    expect(text("#setup-language-label")).toBe("Language");
    expect(text("#setup-continue")).toBe("Continue");
    expect(text(".nav-item[data-view='library']")).toBe("Library");
    expect(text("#library-empty-text")).toBe("No folder");
    expect(el("#replay-search").getAttribute("placeholder")).toBe("Search");
    expect(el("#window-close").getAttribute("aria-label")).toBe("Close");
    expect(text("#storage-root")).toBe(CLIENTS_ROOT);
    // Sigue en la pantalla hasta Continuar, que no vuelve a guardar lo ya elegido.
    expect(el("#splash").hidden).toBe(false);
    click("#setup-continue");
    expect(el("#splash").hidden).toBe(true);
    expect(el("#library-view").inert).toBe(false);
    expect(called("set_language")).toHaveLength(1);

    // En Clientes se cambia con el mismo control que las imágenes.
    click(".nav-item[data-view='clients']");
    expect(text("#language-label")).toBe("App language");
    expect(el("#app-language [data-language='en']").getAttribute("aria-pressed")).toBe("true");
    click("#app-language [data-language='es']");
    await waitFor(() => called("set_language").length === 2, "language changed in Clientes");
    expect(called("set_language")[1].args).toEqual({ language: "es" });
    expect(document.documentElement.lang).toBe("es");
    expect(el("#app-language [data-language='es']").getAttribute("aria-pressed")).toBe("true");
    expect(text("#clients-view h1")).toBe("Clientes");
    expect(text("#builds-list")).toBe("Sin biblioteca");
    click(".nav-item[data-view='library']");
    expect(text("#library-empty-text")).toBe("Sin carpeta");
  });

  test("opening a folder detects the owner and renders the match without any client work", async () => {
    // Sin carpeta recordada la app ya responde.
    expect(el("#splash").hidden).toBe(true);
    expect(el("#library-view").inert).toBe(false);
    // Sin carpeta: solo el estado vacío, sin lista, cuenta ni ruta.
    expect(el("#library-list").hidden).toBe(true);
    expect(el("#library-empty").hidden).toBe(false);
    expect(el("#replay-count").hidden).toBe(true);
    expect(el("#folder-path").hidden).toBe(true);

    // Cambiar de carpeta con una lectura pendiente: al llegar, la repetición anterior no reaparece.
    holdInspect = true;
    dialogAnswers.push(REPLAY_DIR);
    click("#open-library");
    await waitFor(() => heldInspects.length === 3, "held replay reads");
    expect(el("#replay-workspace").hidden).toBe(true);
    dialogAnswers.push(OTHER_DIR);
    click("#open-library");
    await waitFor(() => text("#library-empty-text") === "Sin repeticiones", "empty folder");
    // La pregunta por las imágenes era de la carpeta anterior: una sin parches la retira.
    expect(el("#splash").hidden).toBe(true);
    // El diálogo se abre en la carpeta actual.
    expect(called("plugin:dialog|open").at(-1)?.args.options).toMatchObject({ directory: true, defaultPath: REPLAY_DIR });
    holdInspect = false;
    for (const release of heldInspects.splice(0)) release();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(el("#replay-workspace").hidden).toBe(true);
    expect(text("#empty-title")).toBe("Sin repetición seleccionada");

    // Una carpeta que no se puede leer (un USB desconectado) no reemplaza la recordada y se reintenta.
    folderFailure = "La ruta de repeticiones no es una carpeta";
    dialogAnswers.push(USB_DIR);
    click("#open-library");
    await waitFor(() => text("#library-empty-text").startsWith("No se pudo cargar la carpeta."), "folder error");
    expect(localStorage.getItem("chronorift.lastReplayLibrary")).toBe(OTHER_DIR);
    expect(text("#folder-path")).toBe(USB_DIR);
    folderFailure = null;
    click("#refresh-library");
    await waitFor(() => text("#library-empty-text") === "Sin repeticiones", "retried folder");
    expect(called("inspect_replay_folder").at(-1)?.args.path).toBe(USB_DIR);

    const reads = called("inspect_replay").length;
    dialogAnswers.push(REPLAY_DIR);
    click("#open-library");
    // Primera biblioteca con parches y sin elección guardada: antes de mostrarla se pregunta por las
    // imágenes, con cuántas son y cuánto pesan. Solo cuenta 16.18: 16.17 no tiene datos aquí.
    await waitFor(() => !el("#splash").hidden && !el("#splash-ask").hidden, "prefetch question");
    expect(el("#library-view").inert).toBe(true);
    // El idioma ya se eligió: solo la pregunta por las imágenes.
    expect(el("#setup-language").hidden).toBe(true);
    expect(el("#setup-images").hidden).toBe(false);
    expect(el("#splash-ask").getAttribute("aria-labelledby")).toBe("splash-ask-title");
    expect(text("#splash-ask-data")).toBe("1 parche · 16 imágenes · ~323 KiB");
    click("#prefetch-all");
    await waitFor(() => called("set_asset_prefetch").length === 1, "saved prefetch choice");
    expect(called("set_asset_prefetch")[0].args).toEqual({ enabled: true });
    // El parche entero va en una sola llamada al backend, con las rutas de la caché en disco
    // (`lolasset`), y la pantalla sale al terminar. El parche queda anotado como descargado.
    await waitFor(() => el("#splash").hidden, "prefetch finished");
    expect(el("#library-view").inert).toBe(false);
    expect(called("prefetch_assets")).toHaveLength(1);
    const paths: string[] = called("prefetch_assets")[0].args.paths;
    expect(paths).toHaveLength(16);
    expect(paths[0]).toBe("/ddragon/cdn/16.18.1/img/champion/Urgot.png");
    expect(paths).toContain("/ddragon/cdn/16.18.1/img/item/3020.png");
    expect(paths).toContain("/cdragon/16.18/plugins/rcp-be-lol-game-data/global/default/v1/perk-images/styles/7204_resolve.png");
    expect(JSON.parse(localStorage.getItem("chronorift.prefetchedPatches") ?? "[]")).toEqual(["16.18"]);
    // La interfaz ya no pide imágenes una por una para guardarlas.
    expect(fetched.some((url) => url.endsWith(".png"))).toBe(false);
    // 16.17, sin datos al preguntar, se intenta después en segundo plano: se espera a que termine.
    await waitFor(() => el("#activity").hidden, "background prefetch");
    await waitFor(() => document.querySelectorAll(".replay-row").length === 2, "replay rows");
    await waitFor(() => text("#file-name") === "LA2-1625354615.rofl", "first replay");
    // Cada repetición de la lista se lee de antemano: elegir otra no espera al backend.
    await waitFor(
      () => new Set(called("inspect_replay").slice(reads).map((call) => call.args.path)).size === 2,
      "prefetched replays",
    );

    // El Riot ID presente en todas las partidas es el dueño: victoria en una, derrota en la otra.
    expect(text("#library-owner-name")).toBe(OWNER);
    const rows = [...document.querySelectorAll<HTMLButtonElement>(".replay-row")];
    expect(rows.map((row) => row.querySelector(".result-badge")?.textContent)).toEqual(["V", "D"]);
    // El tono sale del resultado (una clase), no del texto: V en verde y D en rojo, igual que el chip.
    expect(rows.map((row) => row.querySelector(".result-badge")?.classList.contains("is-win"))).toEqual([true, false]);
    expect(rows[1].querySelector(".result-badge")?.classList.contains("is-loss")).toBe(true);
    expect(rows[0].getAttribute("aria-current")).toBe("true");
    expect(rows[0].getAttribute("aria-label")).toContain("9/2/3");
    // El archivo que no se pudo leer queda al final: fila gris con el motivo, sin selección.
    const failed = el("#library-list .failed-row");
    expect(el("#library-list").lastElementChild).toBe(failed);
    expect(failed.tagName).toBe("DIV");
    expect(failed.querySelector(".replay-row__name")?.textContent).toBe("broken.rofl");
    expect(failed.querySelector(".failed-row__reason")?.getAttribute("title")).toBe(library.failures[0].message);
    expect(text("#replay-count")).toBe("3");

    expect(text("#result-chip")).toBe("Victoria");
    expect(el("#result-chip").classList.contains("is-win")).toBe(true);
    expect(text("#result-side")).toBe("Lado rojo");
    expect(text("#blue-score")).toBe("15");
    expect(text("#red-score")).toBe("40");
    expect(el("#red-score").classList.contains("is-winner")).toBe(true);
    expect(text("#duration")).toBe("36:34");
    expect(text("#version")).toBe(BUILD_A);
    expect(text("#match-id")).toBe("1625354615");
    // El ID abre la partida en League of Graphs (región LA2 → las).
    click("#match-id");
    await waitFor(() => called("plugin:opener|open_url").length === 1, "match page");
    expect(called("plugin:opener|open_url")[0].args.url).toBe("https://www.leagueofgraphs.com/match/las/1625354615");
    // Copiar se confirma en el mismo botón: el ícono pasa a una marca, sin texto.
    const { icon } = await import("../src/dom.ts");
    click("#copy-match-id");
    await waitFor(() => el("#copy-match-id svg").innerHTML === icon("check").innerHTML, "copy feedback");
    expect(await navigator.clipboard.readText()).toBe("1625354615");
    await waitFor(() => text("#client-location") === DESTINATION, "local client status");
    expect(text("#client-status-text")).toBe("No instalado");
    expect(text("#client-locale")).toBe("es_MX");
    expect(el("#client-location").classList.contains("is-muted")).toBe(true);
    expect(el("#reveal-client").hidden).toBe(true);
    expect(text("#client-action-label")).toBe("Preparar cliente");
    expect(disabled("#play-replay")).toBe(true);
    // Con el estado leído del disco, la fila y el pie dicen si el cliente está instalado.
    expect(rows[0].getAttribute("aria-label")).toContain("cliente no instalado");
    expect(text("#library-summary")).toBe("0/2 builds · 1 con error");

    // Marcador: dos equipos, el dueño resaltado y objetivos por equipo.
    const teamBlocks = document.querySelectorAll("#teams .sb-team");
    expect(teamBlocks).toHaveLength(2);
    expect(teamBlocks[1].querySelector(".objectives")?.textContent).toContain("Torres9");
    const owner = el("#teams .sb-player.is-owner");
    expect(owner.querySelector(".owner-tag")?.textContent).toBe("Tú");
    expect(owner.querySelector(".kda__line")?.textContent).toBe("9 / 2 / 3");

    // Recursos del parche en el idioma de la interfaz: retratos, hechizos, runas y objetos.
    await waitFor(() => !!document.querySelector("#teams .slot img[alt='Destello']"), "spell icons");
    const wukong = document.querySelectorAll("#teams .sb-player")[1];
    expect(wukong.getAttribute("aria-label")).toContain("Wukong");
    expect(wukong.querySelector<HTMLImageElement>(".slot img[alt='Destello']")?.getAttribute("src")).toBe(
      viaCache(`${CD}/default/data/spells/icons2d/summoner_flash.png`),
    );
    expect(wukong.querySelector(".slot img[alt='Valor']")).not.toBeNull();
    expect(wukong.querySelector(".slot--item")?.getAttribute("title")).toBe("Filo de la tormenta");

    // Con misiones de rol cada fila tiene 8 ranuras: inventario, misión de rol y abalorio.
    const ownerItems = el("#teams .sb-player.is-owner .items");
    expect(ownerItems.querySelectorAll(".slot")).toHaveLength(8);
    expect(ownerItems.querySelector(".slot--role")?.getAttribute("title")).toBe("Botas del Hechicero");
    expect(ownerItems.lastElementChild?.classList.contains("slot--trinket")).toBe(true);

    // Abrir repeticiones nunca planifica, descarga, instala, lanza ni guarda configuración.
    for (const cmd of ["plan_client_install", "install_client", "cancel_client_install", "launch_replay", "set_clients_root"]) {
      expect(called(cmd)).toHaveLength(0);
    }
    expect(called("client_statuses").at(-1)?.args).toEqual({
      clientsRoot: CLIENTS_ROOT,
      versions: [BUILD_A, BUILD_B],
    });
    // Nada va directo a internet: datos e imágenes pasan por la caché en disco.
    expect(fetched.length).toBeGreaterThan(0);
    expect(fetched.every((url) => url.startsWith(`${CACHE}/`))).toBe(true);
    const images = [...document.querySelectorAll<HTMLImageElement>("img")].map((image) => image.getAttribute("src") ?? "");
    expect(images.length).toBeGreaterThan(0);
    expect(images.every((src) => src.startsWith(`${CACHE}/`))).toBe(true);
    // Y la CSP de la app (también la de desarrollo) deja pasar ese origen para imágenes y fetch.
    const security = (await Bun.file(new URL("../src-tauri/tauri.conf.json", import.meta.url)).json()).app.security;
    for (const policy of [security.csp, security.devCsp]) {
      const directives = cspDirectives(policy);
      expect(directives["img-src"]).toContain(CACHE);
      expect(directives["connect-src"]).toContain(CACHE);
    }
  });

  test("search, filters and keyboard selection narrow and keep the library", () => {
    typeInto("#replay-search", "16.17");
    expect(document.querySelectorAll(".replay-row")).toHaveLength(1);
    expect(document.querySelectorAll(".failed-row")).toHaveLength(0);
    expect(text("#replay-count")).toBe("1/3");
    // Los archivos ilegibles se buscan por su nombre.
    typeInto("#replay-search", "broken");
    expect(document.querySelectorAll(".replay-row")).toHaveLength(0);
    expect(document.querySelectorAll(".failed-row")).toHaveLength(1);
    expect(text("#replay-count")).toBe("1/3");
    typeInto("#replay-search", "sin coincidencias");
    expect(el("#library-list").hidden).toBe(true);
    expect(text("#library-empty-text")).toBe("Sin resultados");
    typeInto("#replay-search", "");
    expect(el("#library-list").hidden).toBe(false);

    // Victorias y Derrotas solo muestran partidas del dueño: sin archivos ilegibles.
    click("#result-filter [data-filter='loss']");
    expect(document.querySelectorAll(".replay-row")).toHaveLength(1);
    expect(document.querySelectorAll(".failed-row")).toHaveLength(0);
    expect(text("#result-filter [data-count='loss']")).toBe("1");
    click("#result-filter [data-filter='all']");
    expect(document.querySelectorAll(".replay-row")).toHaveLength(2);
    expect(document.querySelectorAll(".failed-row")).toHaveLength(1);

    // Elegir con Enter (activa el botón con el foco) deja el foco en esa fila: el siguiente Tab no
    // vuelve al principio de la lista.
    const focusedPath = () => (document.activeElement as HTMLElement | null)?.dataset?.path;
    const [first, second] = [...document.querySelectorAll<HTMLButtonElement>(".replay-row")];
    second.focus();
    second.click();
    expect(text("#file-name")).toBe("LA2-1621247134.rofl");
    expect(focusedPath()).toBe(REPLAY_B);
    expect(el(`.replay-row[aria-current='true']`).dataset.path).toBe(REPLAY_B);
    first.focus();
    first.click();
    expect(text("#file-name")).toBe("LA2-1625354615.rofl");
    expect(focusedPath()).toBe(REPLAY_A);
    expect(el(`.replay-row[aria-current='true']`).dataset.path).toBe(REPLAY_A);
  });

  test("stats and file tabs show team totals, damage by type and the replay path", () => {
    click(".tab[data-tab='stats']");
    const compare = [...document.querySelectorAll("#team-compare .compare-row")];
    expect(compare.map((row) => row.querySelector(".compare-row__label")?.textContent)).toContain("Torres");
    const towers = compare.find((row) => row.querySelector(".compare-row__label")?.textContent === "Torres")!;
    expect(towers.querySelectorAll(".compare-row__value")[1].classList.contains("is-lead")).toBe(true);
    expect(document.querySelectorAll("#damage-chart .dmg-row")).toHaveLength(10);
    expect(el("#damage-chart .dmg-row.is-owner").querySelectorAll(".dmg-row__stack span")).toHaveLength(3);
    expect(document.querySelectorAll("#player-table .ptable__row:not(.ptable__row--head)")).toHaveLength(10);

    click(".tab[data-tab='file']");
    expect(text("#file-facts")).toContain(REPLAY_A);
    expect(text("#file-facts")).toContain("ROFL2");
    // La fecha del archivo se llama igual que en el resumen: es cuando se descargó.
    expect(text("#file-facts")).toContain("Descargada");
    click(".tab[data-tab='board']");
    expect(el("#panel-board").hidden).toBe(false);
  });

  test("the plan shows destination, locale, sizes and source before anything is downloaded", async () => {
    click("#client-action");
    await waitFor(() => !el("#client-drawer").hidden, "drawer");
    // El foco entra al panel, un diálogo; la partida de detrás queda inerte.
    expect(document.activeElement?.id).toBe("drawer-close");
    expect(el("#client-drawer").getAttribute("role")).toBe("dialog");
    expect(el("#replay-workspace").inert).toBe(true);
    await waitFor(() => text("#plan-download-size") !== "…", "plan values");

    expect(called("plan_client_install")[0].args.request).toEqual({
      patch: "16.18",
      gameVersion: BUILD_A,
      region: "LA2",
      locale: "es_MX",
      clientsRoot: CLIENTS_ROOT,
      confirmed: null,
    });
    expect(text("#plan-version")).toBe(BUILD_A);
    expect(text("#plan-mode")).toBe("Nueva");
    expect(text("#plan-destination")).toBe(DESTINATION);
    expect(text("#plan-file-count")).toContain("416");
    expect(text("#plan-download-size")).toMatch(/^23[.,]5 GiB$/);
    expect(text("#plan-install-size")).toMatch(/^29[.,]3 GiB$/);
    expect(text("#plan-source")).toBe("riot-manifests");
    expect(text("#confirm-label")).toBe("Descargar");
    expect(called("install_client")).toHaveLength(0);

    // Cambiar el idioma vuelve a planificar: hasta tener el plan nuevo no se puede confirmar el viejo.
    typeInto("#locale", "en_US", ["change"]);
    expect(disabled("#confirm-client-action")).toBe(true);
    await waitFor(() => called("plan_client_install").length === 2, "re-plan");
    expect(called("plan_client_install")[1].args.request.locale).toBe("en_US");
    typeInto("#locale", "es_MX", ["change"]);
    await waitFor(() => called("plan_client_install").length === 3 && !disabled("#confirm-client-action"), "plan ready");
  });

  test("confirming shows real progress and stopping keeps a resumable client", async () => {
    click("#confirm-client-action");
    await waitFor(() => pendingInstall !== null, "install_client call");
    // Se confirma exactamente el plan que se vio, con su idioma y su manifiesto.
    expect(called("install_client")[0].args.request.confirmed).toEqual({
      mode: "install",
      resolvedVersion: "16.18.8175716",
      destinationPath: DESTINATION,
      locale: "es_MX",
      manifestUrl: plan("install", "es_MX").manifestUrl,
    });
    expect(el("#progress-view").hidden).toBe(false);
    expect(el("#activity").hidden).toBe(false);

    // El porcentaje lleva un decimal y la velocidad es el promedio de los últimos segundos: hasta
    // medir un segundo no se muestra (su lugar ya está reservado).
    const start = Date.now();
    setSystemTime(new Date(start));
    await emitProgress("manifest", 0, null, "Downloading manifest");
    await emitProgress("download", 5 * GIB, 25 * GIB, "DATA/FINAL/Champions/Aatrox.es_MX.wad.client");
    await waitFor(() => text("#progress-percent") === "20,0 %", "progress percent");
    expect(text("#progress-file")).toBe("DATA/FINAL/Champions/Aatrox.es_MX.wad.client");
    expect(el("#progress-bar").style.width).toBe("20%");
    expect(el("#stages li[data-stage='manifest']").dataset.status).toBe("done");
    expect(el("#stages li[data-stage='download']").dataset.status).toBe("active");
    expect(text("#activity-percent")).toBe("20,0 %");
    expect(text("#activity-speed")).toBe("—");
    expect(text("#progress-rate")).toBe("");
    expect(text("#client-status-text")).toBe("Descargando · 20,0 %");
    expect(el("#client-progress").hidden).toBe(false);
    expect(el("#client-progress-bar").style.width).toBe("20%");
    expect(text("#client-location")).toBe(DESTINATION);
    // 30,5 MB en 5 s: 6,1 MB/s; faltan 20 GiB menos eso, unos 58,6 min a esa velocidad.
    setSystemTime(new Date(start + 5_000));
    await emitProgress("download", 5 * GIB + 30_500_000, 25 * GIB, "DATA/FINAL/Champions/Ahri.es_MX.wad.client");
    await waitFor(() => text("#activity-speed") === "6,1 MB/s", "download speed");
    setSystemTime();
    expect(text("#progress-percent")).toBe("20,1 %");
    expect(text("#progress-bytes")).toBe("5,03 GiB de 25 GiB");
    expect(text("#progress-rate")).toBe("6,1 MB/s · quedan 59 min");
    expect(text("#client-speed")).toBe("6,1 MB/s");
    expect(text("#client-status-text")).toBe("Descargando · 20,1 %");
    // Sin bytes nuevos (un reintento) no llegan avances: el pulso de cada segundo repite la última
    // muestra y la velocidad baja en lugar de quedar congelada. 30,5 MB en 9 s.
    setSystemTime(new Date(start + 9_000));
    await waitFor(() => text("#activity-speed") === "3,4 MB/s", "speed decays during a stall");
    setSystemTime();

    // Detener se ve detenido en el acto, antes de que el backend termine de cerrar la descarga: sin
    // barra ni indicador, sin "Deteniendo…", y la compilación incompleta con Reanudar.
    holdCancel = true;
    click("#stop-client-action");
    expect(text("#client-status-text")).toBe("Incompleto");
    expect(el("#client-progress").hidden).toBe(true);
    expect(el("#activity").hidden).toBe(true);
    expect(el("#client-drawer").hidden).toBe(true);
    expect(text("#client-action-label")).toBe("Reanudar");
    expect(document.body.textContent).not.toContain("Deteniendo");
    // Bajo la barra, una nota neutra (no un error): lo descargado se conserva y Reanudar sigue.
    expect(el("#client-note").hidden).toBe(false);
    expect(text("#client-note")).toBe("Detenida. Lo descargado se conserva y Reanudar continúa desde ahí.");
    await waitFor(() => called("cancel_client_install").length === 1, "cancel command");
    // Reanudar espera a que el backend cierre la descarga (si no, la rechazaría): el panel se abre,
    // pero el plan se pide recién después, con lo que quedó en disco. Mientras tanto ya dice
    // "Reanudar": no pasa por "Descargar".
    const plans = called("plan_client_install").length;
    click("#client-action");
    expect(el("#client-drawer").hidden).toBe(false);
    expect(text("#drawer-title")).toBe("Reanudar");
    expect(text("#confirm-label")).toBe("Reanudar");
    expect(disabled("#confirm-client-action")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(called("plan_client_install")).toHaveLength(plans);
    holdCancel = false;
    heldCancel!();
    heldCancel = null;
    await waitFor(() => text("#plan-mode") === "Reanudar", "resume plan after the stop");
    expect(called("plan_client_install")).toHaveLength(plans + 1);
    // Lo que hay en disco, también lo guardado a medias, sobre el total: 6 de 29,3 GiB.
    await waitFor(() => text("#client-status-text") === "Incompleto · 20,4 %", "partial status");
    // Una parada pedida no es un error.
    expect(el("#client-error").hidden).toBe(true);
    expect(el("#plan-error").hidden).toBe(true);
    expect(disabled("#play-replay")).toBe(true);
    click("#drawer-close");
    pendingInstall = null;
  });

  test("a failed download stays visible with Reanudar ready; resuming finishes the client, which launches", async () => {
    click("#client-action");
    await waitFor(() => text("#plan-mode") === "Reanudar", "resume plan");
    expect(text("#drawer-title")).toBe("Reanudar");
    expect(text("#confirm-label")).toBe("Reanudar");
    expect(el<HTMLSelectElement>("#locale").value).toBe("es_MX");

    // La descarga falla (tras sus reintentos) con el panel cerrado: el motivo queda bajo la barra del
    // cliente y el plan se rehace solo, así Reanudar abre un plan ya listo.
    click("#confirm-client-action");
    await waitFor(() => pendingInstall !== null, "resume install call");
    click("#drawer-close");
    expect(el("#client-drawer").hidden).toBe(true);
    const plans = called("plan_client_install").length;
    pendingInstall!.reject(NETWORK_FAILURE);
    pendingInstall = null;
    await waitFor(() => text("#client-error") === NETWORK_FAILURE, "visible download error");
    expect(el("#client-drawer").hidden).toBe(true);
    expect(el("#activity").hidden).toBe(true);
    await waitFor(() => called("plan_client_install").length === plans + 1, "automatic re-plan");
    expect(text("#client-status-text")).toBe("Incompleto · 20,4 %");
    expect(text("#client-action-label")).toBe("Reanudar");
    // Un intento nuevo quitó la nota de la parada; el fallo ocupa su lugar.
    expect(el("#client-note").hidden).toBe(true);

    click("#client-action");
    expect(el("#client-drawer").hidden).toBe(false);
    await waitFor(() => !disabled("#confirm-client-action"), "resume plan ready");
    expect(called("plan_client_install")).toHaveLength(plans + 1);
    expect(text("#confirm-label")).toBe("Reanudar");
    // El fallo sigue a la vista hasta el próximo intento, también en el panel.
    expect(text("#plan-error")).toBe(NETWORK_FAILURE);
    expect(text("#client-error")).toBe(NETWORK_FAILURE);

    // Otro idioma en el plan: el backend simulado deja el cliente en es_MX (su install.json), y la
    // barra y el inicio deben usar ese idioma instalado, no el elegido en el plan.
    typeInto("#locale", "en_US", ["change"]);
    await waitFor(
      () => called("plan_client_install").length === plans + 2 && !disabled("#confirm-client-action"),
      "en_US plan",
    );

    click("#confirm-client-action");
    await waitFor(() => pendingInstall !== null, "second resume install call");
    // Un intento nuevo quita el fallo anterior.
    expect(el("#client-error").hidden).toBe(true);
    const retry = called("install_client").at(-1)!.args.request;
    expect(retry.locale).toBe("en_US");
    expect(retry.confirmed).toMatchObject({ mode: "resume", locale: "en_US" });
    await emitProgress("download", 20 * GIB, 25 * GIB, "DATA/FINAL/Maps/Shipping/Map11.wad.client");
    // Detener justo cuando la descarga terminaba bien: se ve detenida un instante y queda "Listo".
    holdCancel = true;
    click("#stop-client-action");
    expect(text("#client-status-text")).toBe("Incompleto");
    disk[BUILD_A] = { state: "ready", installedBytes: 29.3 * GIB, totalInstallBytes: 29.3 * GIB };
    pendingInstall!.resolve({ ...plan("resume", "es_MX"), executablePath: EXECUTABLE, downloadSeconds: 1470 });
    pendingInstall = null;
    holdCancel = false;
    heldCancel = null;
    await waitFor(() => text("#client-status-text") === "Listo", "ready status");
    expect(text("#client-location")).toBe(DESTINATION);
    expect(text("#client-locale")).toBe("es_MX");
    expect(el("#client-action").hidden).toBe(true);
    expect(el("#reveal-client").hidden).toBe(false);
    expect(el("#client-drawer").hidden).toBe(true);
    expect(el("#version-inexact").hidden).toBe(true);
    expect(disabled("#play-replay")).toBe(false);

    // Con Vanguard cargado el estado lo indica, pero Reproducir sigue disponible.
    vanguard = "running";
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => text("#client-status-text") === "Vanguard activo", "vanguard state");
    expect(el("#client-glyph").dataset.state).toBe("blocked");
    expect(disabled("#play-replay")).toBe(false);

    launchFailure = "No se pudo iniciar League of Legends: Acceso denegado. (os error 5)";
    click("#play-replay");
    await waitFor(() => text("#client-error").includes("os error 5"), "blocked launch");
    expect(text("#client-status-text")).toBe("Vanguard activo");
    expect(el("#client-error").hidden).toBe(false);

    // Al volver a la ventana tras detener Vanguard desde la bandeja, el cliente vuelve a estar listo.
    vanguard = "stopped";
    launchFailure = null;
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => text("#client-status-text") === "Listo", "vanguard stopped");
    expect(text("#client-location")).toBe(DESTINATION);
    expect(el("#client-error").hidden).toBe(true);
    expect(el("#client-glyph").dataset.state).toBe("ready");

    click("#play-replay");
    await waitFor(() => called("launch_replay").length === 2, "launch call");
    expect(called("launch_replay")[1].args.request).toEqual({
      replayPath: REPLAY_A,
      executablePath: EXECUTABLE,
      locale: "es_MX",
    });
    await waitFor(() => text("#client-status-text") === "Reproduciendo", "launched");
    // "Reproduciendo" no tapa a Vanguard, y al cerrarse el juego la barra vuelve a la normalidad.
    vanguard = "running";
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => text("#client-status-text") === "Vanguard activo", "vanguard while playing");
    vanguard = "stopped";
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => text("#client-status-text") === "Reproduciendo", "still playing");
    await emitEvent("replay-exited", { replayPath: REPLAY_A, exitCode: 0 });
    await waitFor(() => text("#client-status-text") === "Listo", "game exited");

    // Sin Vanguard, cualquier otro error se muestra tal cual.
    launchFailure = "El ejecutable no existe: E:\\ChronoRift\\gone.exe";
    click("#play-replay");
    await waitFor(() => text("#client-status-text") === "No se pudo reproducir", "launch error");
    expect(text("#client-error")).toContain("gone.exe");
    launchFailure = null;

    click("#reveal-client");
    await waitFor(() => called("plugin:opener|reveal_item_in_dir").length === 1, "reveal call");
    expect(called("plugin:opener|reveal_item_in_dir")[0].args).toEqual({ paths: [EXECUTABLE] });

    // Un "Mostrar en carpeta" fallido se ve en la partida, no solo en la vista Clientes.
    revealFailure = "El sistema no puede encontrar el archivo especificado. (os error 2)";
    click(".replay-caption [data-action='reveal-replay']");
    await waitFor(() => text("#client-error").includes("os error 2"), "reveal error");
    expect(text("#client-error")).toStartWith("No se pudo abrir la carpeta.");
    expect(called("plugin:opener|reveal_item_in_dir")[1].args).toEqual({ paths: [REPLAY_A] });
    expect(text("#client-status-text")).toBe("Listo");
    revealFailure = null;
  });

  test("a build that cannot be resolved is reported; a newer build of its patch is used when installed", async () => {
    planFailure = "Sin manifiesto de lol-game-client para LA2 16.17.8104348 (16.17) en Riot Sieve ni en riot-manifests";
    // Ya precargada: se muestra en el mismo clic, sin estado de carga ni otra lectura.
    const reads = called("inspect_replay").length;
    document.querySelectorAll<HTMLButtonElement>(".replay-row")[1].click();
    expect(text("#file-name")).toBe("LA2-1621247134.rofl");
    expect(el("#replay-workspace").getAttribute("aria-busy")).toBe("false");
    expect(called("inspect_replay")).toHaveLength(reads);
    expect(text("#result-chip")).toBe("Derrota");
    // Sin objetos de misión de rol en la partida no hay ranura para ellos: 6 objetos y el abalorio.
    expect(el("#teams .sb-player .items").querySelectorAll(".slot")).toHaveLength(7);
    expect(document.querySelector("#teams .slot--role")).toBeNull();

    // Un ejecutable que no es el juego se rechaza al elegirlo, con el texto del backend.
    dialogAnswers.push("C:\\Juegos\\Otro\\Juego.exe");
    click("#choose-executable");
    await waitFor(() => !el("#client-error").hidden, "wrong executable");
    expect(text("#client-error")).toBe("No es League of Legends.exe: C:\\Juegos\\Otro\\Juego.exe");
    expect(text("#client-status-text")).toBe("No instalado");
    expect(disabled("#play-replay")).toBe(true);

    click("#client-action");
    await waitFor(() => !el("#plan-error").hidden, "plan error");
    expect(text("#plan-error")).toContain("16.17.8104348");
    expect(disabled("#confirm-client-action")).toBe(true);
    expect(called("install_client")).toHaveLength(3);
    click("#drawer-close");
    // Sin transición en curso el panel se oculta en el acto y el foco vuelve a la barra del cliente.
    expect(el("#client-drawer").hidden).toBe(true);
    expect(el("#drawer-scrim").hidden).toBe(true);
    expect(el("#replay-workspace").inert).toBe(false);
    expect(document.activeElement?.id).toBe("client-action");
    planFailure = null;

    // Con una compilación más nueva del mismo parche instalada, se usa esa y se marca "no exacta".
    disk[BUILD_B] = { state: "ready", installedBytes: 29 * GIB, totalInstallBytes: 29 * GIB, build: BUILD_B_NEWER };
    click("#refresh-library");
    await waitFor(() => text("#client-status-text") === "Listo", "newer build of the patch");
    expect(text("#version")).toBe(BUILD_B_NEWER);
    expect(el("#version-inexact").hidden).toBe(false);
    expect(text("#version-inexact")).toBe("no exacta");
    click("#play-replay");
    await waitFor(() => called("launch_replay").at(-1)?.args.request.replayPath === REPLAY_B, "launch with the newer build");
    expect(called("launch_replay").at(-1)?.args.request.executablePath).toBe(
      `${CLIENTS_ROOT}\\${BUILD_B_NEWER}\\Game\\League of Legends.exe`,
    );
    await emitEvent("replay-exited", { replayPath: REPLAY_B, exitCode: 0 });
    disk[BUILD_B] = { state: "missing", installedBytes: 0, totalInstallBytes: null };
    click("#refresh-library");
    await waitFor(() => text("#client-status-text") === "No instalado", "newer build removed");
    expect(el("#version-inexact").hidden).toBe(true);
  });

  test("the clients view lists builds, saves the downloads folder and reports an unavailable drive", async () => {
    click(".nav-item[data-view='clients']");
    await waitFor(() => called("list_clients").length > 0, "list_clients call");
    await waitFor(() => document.querySelectorAll("#builds-list .builds__row").length === 2, "build rows");
    const rows = [...document.querySelectorAll<HTMLElement>("#builds-list .builds__row")];
    expect(rows.map((row) => row.dataset.build)).toEqual([BUILD_A, BUILD_B]);
    expect(rows[0].querySelector(".builds__state")?.textContent).toBe("Listo");
    expect(rows[1].querySelector(".builds__state")?.textContent).toBe("No instalado");
    expect(text("#clients-installed")).toBe("1/2");

    // La elección de la primera vez se ve aquí y se puede cambiar; se guarda con el mismo comando.
    const choice = (value: string) => el(`#asset-prefetch [data-prefetch='${value}']`).getAttribute("aria-pressed");
    expect([choice("all"), choice("open")]).toEqual(["true", "false"]);
    click("#asset-prefetch [data-prefetch='open']");
    await waitFor(() => called("set_asset_prefetch").length === 2, "changed prefetch choice");
    expect(called("set_asset_prefetch")[1].args).toEqual({ enabled: false });
    expect([choice("all"), choice("open")]).toEqual(["false", "true"]);

    // Una compilación incompleta muestra lo que ya tiene en disco y de cuánto, no el total.
    disk[BUILD_B] = { state: "partial", installedBytes: 12.3 * GIB, totalInstallBytes: 29.2 * GIB };
    click("#refresh-library");
    await waitFor(() => text(`#builds-list [data-build='${BUILD_B}'] .builds__state`) === "Incompleto · 42,1 %", "partial build");
    expect(text(`#builds-list [data-build='${BUILD_B}'] .builds__size`)).toBe("12,3 GiB de 29,2 GiB");
    expect([...document.querySelectorAll(`#builds-list [data-build='${BUILD_B}'] .builds__actions button`)].map((button) => button.textContent))
      .toEqual(["Reanudar", "Abrir"]);
    disk[BUILD_B] = { state: "missing", installedBytes: 0, totalInstallBytes: null };
    click("#refresh-library");
    await waitFor(() => text(`#builds-list [data-build='${BUILD_B}'] .builds__state`) === "No instalado", "missing again");

    typeInto("#install-folder", "E:\\", ["change"]);
    await waitFor(() => settings.clientsRoot === "E:\\ChronoRift", "normalized folder");
    expect(called("set_clients_root").at(-1)?.args).toEqual({ path: "E:\\" });
    await waitFor(() => el<HTMLInputElement>("#install-folder").value === "E:\\ChronoRift", "field value");

    // La carpeta elegida está en un USB desconectado: la lectura falla con el texto del backend.
    unavailableRoot = "D:\\Clientes LoL";
    dialogAnswers.push("D:\\Clientes LoL");
    click("#choose-install-folder");
    await waitFor(() => el<HTMLInputElement>("#install-folder").value === "D:\\Clientes LoL", "picked folder");
    expect(called("plugin:dialog|open").at(-1)?.args.options).toMatchObject({ directory: true, defaultPath: "E:\\ChronoRift" });
    expect(text("#storage-root")).toBe("D:\\Clientes LoL");
    const drive = "La carpeta de clientes no está disponible: D:\\Clientes LoL";
    await waitFor(() => text("#clients-root-error") === drive, "unavailable drive");
    expect(text("#clients-installed")).toBe("—");

    // En la partida: sin "No instalado" ni un "Listo" viejo, y sin poder reproducir.
    click(".nav-item[data-view='library']");
    expect(el("#library-view").hidden).toBe(false);
    expect(text("#client-error")).toBe(drive);
    expect(text("#client-status-text")).toBe("No disponible");
    expect(el("#client-action").hidden).toBe(true);
    expect(disabled("#play-replay")).toBe(true);

    // La unidad vuelve: al regresar a la ventana se relee y el aviso desaparece.
    unavailableRoot = null;
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => el("#client-error").hidden, "drive back");
    expect(text("#client-status-text")).toBe("No instalado");
  });

  test("switching to English with a match open re-renders it, with English names and number formats", async () => {
    document.querySelectorAll<HTMLButtonElement>(".replay-row")[0].click();
    await waitFor(() => text("#file-name") === "LA2-1625354615.rofl", "replay A");
    await waitFor(() => el("#teams .slot--item").getAttribute("title") === "Filo de la tormenta", "Spanish names");
    const kdaRatio = () => text("#teams .sb-player.is-owner .kda__ratio");
    expect(kdaRatio()).toBe("6,00");

    click(".nav-item[data-view='clients']");
    click("#app-language [data-language='en']");
    click(".nav-item[data-view='library']");
    await waitFor(() => called("set_language").at(-1)?.args.language === "en", "saved English");
    // Textos, formatos y resultado en inglés en el acto; los nombres de campeones, objetos y runas
    // llegan con los datos en_US del parche, que se piden ahora.
    expect(text("#result-chip")).toBe("Victory");
    expect(text(".tab[data-tab='board']")).toBe("Scoreboard");
    expect(text("#client-status-text")).toBe("Ready");
    expect(text("#play-replay")).toBe("Play");
    expect(kdaRatio()).toBe("6.00");
    expect(text("#teams .sb-player.is-owner .owner-tag")).toBe("You");
    expect([...document.querySelectorAll(".replay-row .result-badge")].map((badge) => badge.textContent)).toEqual(["W", "L"]);
    expect(text("#library-summary")).toBe("1/2 builds · 1 with errors");
    await waitFor(() => el("#teams .slot--item").getAttribute("title") === "Stormsurge", "English item names");
    expect(document.querySelector("#teams .slot img[alt='Flash']")).not.toBeNull();
    expect(fetched).toContain(viaCache(`${DD}/cdn/16.18.1/data/en_US/item.json`));
    // Las imágenes no dependen del idioma: el parche sigue anotado como descargado.
    expect(JSON.parse(localStorage.getItem("chronorift.prefetchedPatches") ?? "[]")).toEqual(["16.18"]);
    expect(called("prefetch_assets")).toHaveLength(1);
  });
});
