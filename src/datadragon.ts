// Static game data from Riot DataDragon. Only static JSON and images are
// fetched here; this never touches game-client files.

const CDN = "https://ddragon.leagueoflegends.com";

export type Asset = { id: string; name: string; image: string };

export type DataDragonAssets = {
  version: string;
  locale: string;
  champions: Map<string, Asset>;
  items: Map<string, Asset>;
  /** Keyed by the numeric summoner spell key, e.g. "4" for Flash. */
  spells: Map<string, Asset>;
  /** Rune styles and runes keyed by numeric id, e.g. "8000" Precision, "8005" Press the Attack. */
  runes: Map<string, Asset>;
};

type ChampionJson = {
  data: Record<string, { id: string; key: string; name: string; image: { full: string } }>;
};
type ItemJson = { data: Record<string, { name: string; image: { full: string } }> };
type SummonerJson = {
  data: Record<string, { id: string; key: string; name: string; image: { full: string } }>;
};

type RunesJson = {
  id: number;
  key: string;
  name: string;
  icon: string;
  slots: { runes: { id: number; key: string; name: string; icon: string }[] }[];
}[];

let versions: Promise<string[]> | null = null;
const assetCache = new Map<string, Promise<DataDragonAssets>>();

function fetchVersions(): Promise<string[]> {
  versions ??= fetch(`${CDN}/api/versions.json`)
    .then((response) => {
      if (!response.ok) throw new Error("DataDragon versions unavailable");
      return response.json() as Promise<string[]>;
    })
    .catch((error) => {
      versions = null;
      throw error;
    });
  return versions;
}

/** Maps "14.8" or "14.8.577.1234" to the newest DataDragon release of that patch. */
export async function resolveVersion(versionOrPatch: string): Promise<string> {
  const patch = versionOrPatch.split(".").slice(0, 2).join(".");
  try {
    const list = await fetchVersions();
    return list.find((candidate) => candidate.startsWith(`${patch}.`)) ?? versionOrPatch;
  } catch {
    return versionOrPatch;
  }
}

export async function loadAssets(versionOrPatch: string, locale: string): Promise<DataDragonAssets> {
  const version = await resolveVersion(versionOrPatch);
  const key = `${version}:${locale}`;
  let cached = assetCache.get(key);
  if (!cached) {
    cached = fetchAssets(version, locale);
    cached.catch(() => assetCache.delete(key));
    assetCache.set(key, cached);
  }
  return cached;
}

async function fetchAssets(version: string, locale: string): Promise<DataDragonAssets> {
  const [champions, items, spells, runes] = await Promise.all([
    fetchJson<ChampionJson>(version, locale, "champion.json"),
    fetchJson<ItemJson>(version, locale, "item.json"),
    fetchJson<SummonerJson>(version, locale, "summoner.json").catch(() => null),
    fetchJson<RunesJson>(version, locale, "runesReforged.json").catch(() => null),
  ]);
  return {
    version,
    locale,
    champions: championMap(version, champions),
    items: itemMap(version, items),
    spells: spells ? spellMap(version, spells) : new Map(),
    runes: runes ? runeMap(runes) : new Map(),
  };
}

async function fetchJson<T>(version: string, locale: string, file: string): Promise<T> {
  const locales = locale === "en_US" ? ["en_US"] : [locale, "en_US"];
  for (const candidate of locales) {
    const response = await fetch(`${CDN}/cdn/${version}/data/${candidate}/${file}`);
    if (response.ok) return (await response.json()) as T;
  }
  throw new Error(`${file} missing for ${version}`);
}

function championMap(version: string, json: ChampionJson): Map<string, Asset> {
  const map = new Map<string, Asset>();
  for (const champion of Object.values(json.data)) {
    const asset = {
      id: champion.id,
      name: champion.name,
      image: `${CDN}/cdn/${version}/img/champion/${champion.image.full}`,
    };
    map.set(normalizeKey(champion.id), asset);
    map.set(normalizeKey(champion.name), asset);
    map.set(champion.key, asset);
  }
  return map;
}

function itemMap(version: string, json: ItemJson): Map<string, Asset> {
  const map = new Map<string, Asset>();
  for (const [id, item] of Object.entries(json.data)) {
    map.set(id, { id, name: item.name, image: `${CDN}/cdn/${version}/img/item/${item.image.full}` });
  }
  return map;
}

function spellMap(version: string, json: SummonerJson): Map<string, Asset> {
  const map = new Map<string, Asset>();
  for (const spell of Object.values(json.data)) {
    map.set(spell.key, {
      id: spell.id,
      name: spell.name,
      image: `${CDN}/cdn/${version}/img/spell/${spell.image.full}`,
    });
  }
  return map;
}

function runeMap(json: RunesJson): Map<string, Asset> {
  const map = new Map<string, Asset>();
  for (const style of json) {
    map.set(String(style.id), { id: style.key, name: style.name, image: `${CDN}/cdn/img/${style.icon}` });
    for (const slot of style.slots) {
      for (const rune of slot.runes) {
        map.set(String(rune.id), { id: rune.key, name: rune.name, image: `${CDN}/cdn/img/${rune.icon}` });
      }
    }
  }
  return map;
}

export function normalizeKey(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}

export function findChampion(
  champions: Map<string, Asset> | null | undefined,
  champion: string,
): Asset | null {
  return champions?.get(normalizeKey(champion)) ?? champions?.get(champion) ?? null;
}

let latestChampions: Promise<Map<string, Asset>> | null = null;

/**
 * Champion icons for the library list: one en_US table from the newest
 * DataDragon release serves every row (champion ids are stable across patches).
 */
export function loadChampionIndex(): Promise<Map<string, Asset>> {
  latestChampions ??= (async () => {
    const latest = (await fetchVersions())[0];
    return championMap(latest, await fetchJson<ChampionJson>(latest, "en_US", "champion.json"));
  })().catch((error) => {
    latestChampions = null;
    throw error;
  });
  return latestChampions;
}
