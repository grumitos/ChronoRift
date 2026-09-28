// Public profile links built from Riot ID + platform. Opened only on click.

const REGION_SLUGS: Record<string, string> = {
  NA1: "na",
  EUW1: "euw",
  EUN1: "eune",
  KR: "kr",
  BR1: "br",
  LA1: "lan",
  LA2: "las",
  JP1: "jp",
  OC1: "oce",
  RU: "ru",
  TR1: "tr",
  TW2: "tw",
  VN2: "vn",
  SG2: "sg",
  PH2: "ph",
  TH2: "th",
  ME1: "me",
};

export function regionSlug(region: string | null | undefined): string | null {
  if (!region) return null;
  return REGION_SLUGS[region.trim().toUpperCase()] ?? null;
}

export type ProfileLink = { site: string; url: string };

export function profileLinks(
  name: string | null | undefined,
  tagLine: string | null | undefined,
  region: string | null | undefined,
): ProfileLink[] {
  const slug = regionSlug(region);
  if (!slug || !name?.trim() || !tagLine?.trim()) return [];
  const riotId = `${encodeURIComponent(name.trim())}-${encodeURIComponent(tagLine.trim())}`;
  return [
    { site: "op.gg", url: `https://www.op.gg/summoners/${slug}/${riotId}` },
    { site: "League of Graphs", url: `https://www.leagueofgraphs.com/summoner/${slug}/${riotId}` },
  ];
}
