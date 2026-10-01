export type MatchAsset = {
  id: string;
  name: string;
  description: string;
  iconUrl: string | null;
};

export type CommunityDragonAssets = {
  spells: Map<string, MatchAsset>;
  perks: Map<string, MatchAsset>;
  styles: Map<string, MatchAsset>;
};

export type ImageAsset = {
  id: string;
  name: string;
  iconUrl: string;
};

export type DataDragonAssets = {
  version: string;
  champions: Map<string, ImageAsset>;
  items: Map<string, ImageAsset>;
};

type DataDragonChampion = { id: string; name: string; image: { full: string } };
type DataDragonItem = { name: string; image: { full: string } };

type CommunityDragonSpell = {
  id: number;
  name: string;
  description?: string;
  iconPath?: string;
};

type CommunityDragonPerk = {
  id: number;
  name: string;
  shortDesc?: string;
  longDesc?: string;
  iconPath?: string;
};

type CommunityDragonStyle = {
  id: number;
  name: string;
  iconPath?: string;
};

type CommunityDragonStyles = { styles?: CommunityDragonStyle[] } | CommunityDragonStyle[];

const RAW_ROOT = "https://raw.communitydragon.org";
const DATA_DRAGON_ROOT = "https://ddragon.leagueoflegends.com";
const CLIENT_ASSET_PREFIX = "/lol-game-data/assets";
// Orígenes que pasan por la caché en disco del backend (esquema `lolasset`).
const CACHED_ORIGINS: Array<[source: string, origin: string]> = [
  ["ddragon", DATA_DRAGON_ROOT],
  ["cdragon", RAW_ROOT],
];

/**
 * Dentro de la app, imágenes y datos de DataDragon y CommunityDragon se piden a la caché en disco:
 * cada archivo se descarga una sola vez. Fuera de Tauri se usa la URL original.
 */
export function cachedAssetUrl(url: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return url;
  const match = CACHED_ORIGINS.find(([, origin]) => url.startsWith(`${origin}/`));
  if (!match) return url;
  // WebView2 expone los esquemas propios como http://<esquema>.localhost.
  const base = navigator.userAgent.includes("Windows") ? "http://lolasset.localhost" : "lolasset://localhost";
  return `${base}/${match[0]}${url.slice(match[1].length)}`;
}

/**
 * Ruta de un archivo dentro del esquema `lolasset`, la que recibe `prefetch_assets`: la de la URL
 * de la caché (`/ddragon/cdn/…` o `/cdragon/…`), con los `%XX` tal cual. Otro origen, `null`.
 */
export function cachedAssetPath(url: string) {
  const match = CACHED_ORIGINS.find(([, origin]) => url.startsWith(`${origin}/`));
  if (!match) return null;
  try {
    return `/${match[0]}${new URL(url).pathname}`;
  } catch {
    return null;
  }
}

export function localeCandidates(locale: string) {
  const normalized = locale.trim().toLowerCase();
  if (normalized === "default" || !/^[a-z]{2}_[a-z]{2}$/.test(normalized)) {
    return ["default"];
  }
  return [normalized, "default"];
}

export function communityDragonAssetUrl(patch: string, iconPath: string) {
  if (!isPatch(patch) || !iconPath.startsWith(`${CLIENT_ASSET_PREFIX}/`)) return null;
  const suffix = iconPath.slice(CLIENT_ASSET_PREFIX.length).toLowerCase();
  return `${RAW_ROOT}/${patch}/plugins/rcp-be-lol-game-data/global/default${encodeURI(suffix)}`;
}

export function mapCommunityDragonAssets(
  patch: string,
  spells: CommunityDragonSpell[],
  perks: CommunityDragonPerk[],
  styles: CommunityDragonStyles | null = null,
): CommunityDragonAssets {
  const styleList = Array.isArray(styles) ? styles : styles?.styles ?? [];
  return {
    styles: new Map(
      styleList.map((style) => [
        String(style.id),
        {
          id: String(style.id),
          name: style.name,
          description: "",
          iconUrl: communityDragonAssetUrl(patch, style.iconPath ?? ""),
        },
      ]),
    ),
    spells: new Map(
      spells.map((spell) => [
        String(spell.id),
        {
          id: String(spell.id),
          name: spell.name,
          description: spell.description ?? "",
          iconUrl: communityDragonAssetUrl(patch, spell.iconPath ?? ""),
        },
      ]),
    ),
    perks: new Map(
      perks.map((perk) => [
        String(perk.id),
        {
          id: String(perk.id),
          name: perk.name,
          description: perk.shortDesc ?? perk.longDesc ?? "",
          iconUrl: communityDragonAssetUrl(patch, perk.iconPath ?? ""),
        },
      ]),
    ),
  };
}

export function mapDataDragonAssets(
  version: string,
  champions: Record<string, DataDragonChampion>,
  items: Record<string, DataDragonItem>,
): DataDragonAssets {
  const championMap = new Map<string, ImageAsset>();
  for (const champion of Object.values(champions)) {
    const asset = {
      id: champion.id,
      name: champion.name,
      iconUrl: `${DATA_DRAGON_ROOT}/cdn/${version}/img/champion/${champion.image.full}`,
    };
    championMap.set(normalizeKey(champion.id), asset);
    championMap.set(normalizeKey(champion.name), asset);
  }

  const itemMap = new Map<string, ImageAsset>();
  for (const [id, item] of Object.entries(items)) {
    itemMap.set(id, {
      id,
      name: item.name,
      iconUrl: `${DATA_DRAGON_ROOT}/cdn/${version}/img/item/${item.image.full}`,
    });
  }
  return { version, champions: championMap, items: itemMap };
}

export type AssetKind = "champion" | "item" | "spell" | "perk" | "style";
export type PatchImage = { url: string; kind: AssetKind };

/**
 * Bytes promedio de cada tipo de ícono, medidos en el parche 16.18 (16.18.1) el 2026-09-30: 173
 * campeones (4,6 MiB), 868 objetos (5,5 MiB), 34 hechizos, 83 runas (1,9 MiB) y 5 ramas; 1163
 * íconos y 12,2 MiB en total, más 1 MiB de JSON. Estiman el tamaño de un parche antes de pedirlo.
 */
export const AVERAGE_ICON_BYTES: Record<AssetKind, number> = {
  champion: 28_128,
  item: 6_686,
  spell: 5_640,
  perk: 23_670,
  style: 1_080,
};

/**
 * Íconos que la vista de partida puede mostrar para un parche, sin repetir URL: campeones y objetos
 * de DataDragon; hechizos, runas y ramas de CommunityDragon. Si falta un lado, solo el otro.
 */
export function patchImages(
  data: DataDragonAssets | null,
  community: CommunityDragonAssets | null,
): PatchImage[] {
  const images = new Map<string, AssetKind>();
  const add = (assets: Iterable<{ iconUrl: string | null }> | undefined, kind: AssetKind) => {
    for (const { iconUrl } of assets ?? []) if (iconUrl && !images.has(iconUrl)) images.set(iconUrl, kind);
  };
  add(data?.champions.values(), "champion");
  add(data?.items.values(), "item");
  add(community?.spells.values(), "spell");
  add(community?.perks.values(), "perk");
  add(community?.styles.values(), "style");
  return [...images].map(([url, kind]) => ({ url, kind }));
}

export async function loadDataDragonAssets(
  requestedVersion: string,
  locale: string,
): Promise<DataDragonAssets> {
  const version = await resolveDataDragonVersion(requestedVersion);
  const [champions, items] = await Promise.all([
    fetchDataDragonFile<{ data: Record<string, DataDragonChampion> }>(
      version,
      locale,
      "champion.json",
    ),
    fetchDataDragonFile<{ data: Record<string, DataDragonItem> }>(version, locale, "item.json"),
  ]);
  return mapDataDragonAssets(version, champions.data, items.data);
}

async function resolveDataDragonVersion(requestedVersion: string) {
  const patch = requestedVersion.split(".").slice(0, 2).join(".");
  try {
    const response = await fetch(cachedAssetUrl(`${DATA_DRAGON_ROOT}/api/versions.json`));
    if (!response.ok) return requestedVersion;
    const versions = (await response.json()) as string[];
    return versions.find((candidate) => candidate.startsWith(`${patch}.`)) ?? requestedVersion;
  } catch {
    return requestedVersion;
  }
}

async function fetchDataDragonFile<T>(version: string, locale: string, file: string): Promise<T> {
  const candidates = /^[a-z]{2}_[A-Z]{2}$/.test(locale) ? [locale, "en_US"] : ["en_US"];
  for (const candidate of [...new Set(candidates)]) {
    const response = await fetch(
      cachedAssetUrl(`${DATA_DRAGON_ROOT}/cdn/${version}/data/${candidate}/${file}`),
    );
    if (response.ok) return (await response.json()) as T;
  }
  throw new Error(`DataDragon ${file} unavailable for ${version}`);
}

export async function loadCommunityDragonAssets(
  patch: string,
  locale: string,
): Promise<CommunityDragonAssets> {
  if (!isPatch(patch)) throw new Error(`Invalid CommunityDragon patch: ${patch}`);
  const [spells, perks, styles] = await Promise.all([
    fetchDataset<CommunityDragonSpell[]>(patch, locale, "summoner-spells.json"),
    fetchDataset<CommunityDragonPerk[]>(patch, locale, "perks.json"),
    // Opcional: sin estilos, la rama secundaria se muestra por su id.
    fetchDataset<CommunityDragonStyles>(patch, locale, "perkstyles.json").catch(() => null),
  ]);
  return mapCommunityDragonAssets(patch, spells, perks, styles);
}

async function fetchDataset<T>(patch: string, locale: string, file: string): Promise<T> {
  const failures: string[] = [];
  for (const candidate of localeCandidates(locale)) {
    const url = `${RAW_ROOT}/${patch}/plugins/rcp-be-lol-game-data/global/${candidate}/v1/${file}`;
    try {
      const response = await fetch(cachedAssetUrl(url));
      if (response.ok) return (await response.json()) as T;
      failures.push(`${candidate}: HTTP ${response.status}`);
    } catch (error) {
      failures.push(`${candidate}: ${String(error)}`);
    }
  }
  throw new Error(`CommunityDragon ${file} unavailable (${failures.join(", ")})`);
}

function isPatch(value: string) {
  return /^\d+\.\d+$/.test(value);
}

export function normalizeKey(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}
