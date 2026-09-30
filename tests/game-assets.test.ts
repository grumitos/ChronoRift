import { afterEach, describe, expect, test } from "bun:test";
import {
  communityDragonAssetUrl,
  loadCommunityDragonAssets,
  loadDataDragonAssets,
} from "../src/game-assets";

const DD = "https://ddragon.leagueoflegends.com";
const CD = "https://raw.communitydragon.org/16.18/plugins/rcp-be-lol-game-data/global";
const realFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = realFetch;
});

// Sirve JSON fijo por URL y registra cada petición; lo demás responde 404 como el CDN.
function serveStaticData(routes: Record<string, unknown>) {
  const requested: string[] = [];
  globalThis.fetch = (async (input: string | URL | Request) => {
    const url = input instanceof Request ? input.url : String(input);
    requested.push(url);
    return url in routes
      ? new Response(JSON.stringify(routes[url]), { status: 200 })
      : new Response("not found", { status: 404 });
  }) as typeof fetch;
  return requested;
}

describe("patch-scoped match assets", () => {
  test("DataDragon resolves the newest build of the replay patch and falls back to en_US per file", async () => {
    const requested = serveStaticData({
      [`${DD}/api/versions.json`]: ["16.19.1", "16.18.2", "16.18.1", "16.17.1"],
      [`${DD}/cdn/16.18.2/data/en_US/champion.json`]: {
        data: { MonkeyKing: { id: "MonkeyKing", name: "Wukong", image: { full: "MonkeyKing.png" } } },
      },
      [`${DD}/cdn/16.18.2/data/es_MX/item.json`]: {
        data: { "3508": { name: "Filo de la tormenta", image: { full: "3508.png" } } },
      },
    });

    const assets = await loadDataDragonAssets("16.18.1", "es_MX");

    expect(assets.version).toBe("16.18.2");
    // Las repeticiones usan el id interno (MonkeyKing); la UI también busca por nombre visible.
    expect(assets.champions.get("monkeyking")?.iconUrl).toBe(
      `${DD}/cdn/16.18.2/img/champion/MonkeyKing.png`,
    );
    expect(assets.champions.get("wukong")?.name).toBe("Wukong");
    expect(assets.items.get("3508")?.name).toBe("Filo de la tormenta");
    expect(requested).toContain(`${DD}/cdn/16.18.2/data/es_MX/champion.json`);
    expect(requested).toContain(`${DD}/cdn/16.18.2/data/en_US/champion.json`);
  });

  test("CommunityDragon falls back to the neutral locale and maps client paths to lowercase URLs", async () => {
    const requested = serveStaticData({
      [`${CD}/default/v1/summoner-spells.json`]: [
        {
          id: 4,
          name: "Flash",
          description: "Teleport a short distance.",
          iconPath: "/lol-game-data/assets/DATA/Spells/Icons2D/Summoner_Flash.png",
        },
      ],
      [`${CD}/es_mx/v1/perks.json`]: [
        {
          id: 8005,
          name: "Ataque intensificado",
          shortDesc: "Tres ataques seguidos.",
          iconPath: "/lol-game-data/assets/v1/perk-images/Styles/Precision/PressTheAttack/PressTheAttack.png",
        },
      ],
    });

    const assets = await loadCommunityDragonAssets("16.18", "es_MX");

    expect(assets.spells.get("4")).toEqual({
      id: "4",
      name: "Flash",
      description: "Teleport a short distance.",
      iconUrl: `${CD}/default/data/spells/icons2d/summoner_flash.png`,
    });
    expect(assets.perks.get("8005")?.iconUrl).toBe(
      `${CD}/default/v1/perk-images/styles/precision/presstheattack/presstheattack.png`,
    );
    expect(requested).toContain(`${CD}/es_mx/v1/summoner-spells.json`);
    expect(communityDragonAssetUrl("16.18", "https://example.com/icon.png")).toBeNull();

    // Sin datos para el parche, el error nombra cada intento en lugar de fallar en silencio.
    serveStaticData({});
    await expect(loadCommunityDragonAssets("16.18", "es_MX")).rejects.toThrow(
      "es_mx: HTTP 404, default: HTTP 404",
    );
    await expect(loadCommunityDragonAssets("latest", "es_MX")).rejects.toThrow("Invalid");
  });
});
