/**
 * The business catalog domain: catalog, collections, orders, the profile delta
 * and the cover photo.
 *
 * Visitor catalog requests use an in-process HTTP fixture. The remaining calls
 * check input validation and typed results without a connected peer.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

async function offlineClient() {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    createHttp()
  );
}

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string };
  }
  throw new Error("expected the call to reject");
}

const BIZ = "5511999999999@s.whatsapp.net";

describe("catalog reads", () => {
  test("visitor tokens and options reach the catalog HTTP request", async () => {
    type RequestBody = {
      access_token: string;
      variables: { request: { product_catalog?: { limit: string }; collections?: { collection_limit: string } } };
    };
    const requests: Array<{ url: string; body: RequestBody }> = [];
    const client = await createWhatsAppClient(
      { connect() {}, send() {}, disconnect() {} },
      {
        async execute(url, _method, _headers, body) {
          requests.push({ url, body: JSON.parse(new TextDecoder().decode(body!)) });
          return {
            statusCode: 200,
            body: new TextEncoder().encode(JSON.stringify({
              data: {
                xwa_product_catalog_get_product_catalog: {
                  product_catalog: { products: [], paging: {} },
                },
                xwa_product_catalog_get_collections: { collections: [], paging: {} },
              },
            })),
          };
        },
      }
    );
    try {
      const options = { catalogVisitorToken: "visitor-token", after: "cursor-1" };
      const catalog = await client.getCatalog(BIZ, { ...options, limit: 5 });
      const collections = await client.getCollections(BIZ, { ...options, collectionLimit: 3 });
      expect(catalog.products).toEqual([]);
      expect(collections.collections).toEqual([]);
      expect(requests).toHaveLength(2);
      for (const request of requests) {
        expect(request.url).toBe("https://graph.whatsapp.com/graphql/catalog");
        expect(request.body.access_token).toBe("visitor-token");
      }
      expect(requests[0].body.variables.request.product_catalog?.limit).toBe("5");
      expect(requests[1].body.variables.request.collections?.collection_limit).toBe("3");
    } finally {
      client.free();
    }
  }, 20000);

  test("every read reaches the core", async () => {
    const client = await offlineClient();
    try {
      const calls: Array<[string, Promise<unknown>]> = [
        ["getOrder", client.getOrder(BIZ, "ORDER1", "TOKEN1")],
        ["removeBusinessCoverPhoto", client.removeBusinessCoverPhoto("fbid-1")],
      ];

      for (const [name, call] of calls) {
        const error = await rejection(call);
        expect(error.kind, name).toBe("not-connected");
      }
    } finally {
      client.free();
    }
  }, 30000);

  test("visitor reads require a nonempty token", async () => {
    const client = await offlineClient();
    try {
      const withOptions = await rejection(
        client.getCatalog(BIZ, {
          limit: 5,
          after: "cursor-1",
          imageWidth: 200,
          imageHeight: 200,
          allowShopSource: false,
        })
      );
      expect(withOptions.kind).toBe("invalid-argument");
      expect(withOptions.message).toContain("options.catalogVisitorToken");

      const collections = await rejection(
        client.getCollections(BIZ, { collectionLimit: 3, itemLimit: 4, after: "cursor-2", catalogVisitorToken: " " })
      );
      expect(collections.kind).toBe("invalid-argument");
      expect(collections.message).toContain("options.catalogVisitorToken");
    } finally {
      client.free();
    }
  }, 20000);

  test("a malformed JID is rejected before the core is reached", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.getCatalog("not a jid"));
      expect(error.kind).toBe("invalid-argument");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("profile delta", () => {
  test("a delta with fields set reaches the core", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        client.updateBusinessProfile({
          description: "We sell things",
          websites: ["https://example.com"],
          businessHours: {
            timezone: "America/Araguaina",
            config: [
              { dayOfWeek: "mon", mode: "specific_hours", openTime: 540, closeTime: 1080 },
              { dayOfWeek: "sun", mode: "appointment_only" },
            ],
          },
        })
      );
      expect(error.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);

  test("the core rejects an empty delta and too many websites", async () => {
    const client = await offlineClient();
    try {
      // Reaching these proves the delta crossed: they are the core's own
      // pre-flight checks, which run before any I/O.
      // The delta is caller input, so its validation failures classify as
      // invalid arguments rather than as internal faults.
      const empty = await rejection(client.updateBusinessProfile({}));
      expect(empty.message).toContain("empty");
      expect(empty.kind).toBe("invalid-argument");

      const tooMany = await rejection(
        client.updateBusinessProfile({
          websites: ["https://a.example", "https://b.example", "https://c.example"],
        })
      );
      expect(tooMany.message).toContain("websites");
      expect(tooMany.kind).toBe("invalid-argument");
    } finally {
      client.free();
    }
  }, 20000);

  test("business hours validation runs on the values that crossed", async () => {
    const client = await offlineClient();
    try {
      // 1500 is past the end of the day. The core names the day and field,
      // which only works if both reached it.
      const error = await rejection(
        client.updateBusinessProfile({
          businessHours: {
            config: [
              { dayOfWeek: "tue", mode: "specific_hours", openTime: 540, closeTime: 1500 },
            ],
          },
        })
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("tue");
      expect(error.message).toContain("minute of the day");
    } finally {
      client.free();
    }
  }, 20000);

  test("a half-specified opening range is refused, not quietly completed", async () => {
    const client = await offlineClient();
    try {
      // The core's constructors admit no half-set range, so the endpoint would
      // otherwise be dropped and the day applied with hours the caller never
      // asked for.
      const openOnly = await rejection(
        client.updateBusinessProfile({
          businessHours: {
            config: [{ dayOfWeek: "wed", mode: "specific_hours", openTime: 540 }],
          },
        })
      );
      expect(openOnly.kind).toBe("invalid-argument");
      expect(openOnly.message).toContain("wed");
      expect(openOnly.message).toContain("closeTime");

      const closeOnly = await rejection(
        client.updateBusinessProfile({
          businessHours: {
            config: [{ dayOfWeek: "thu", mode: "specific_hours", closeTime: 1080 }],
          },
        })
      );
      expect(closeOnly.kind).toBe("invalid-argument");
      expect(closeOnly.message).toContain("thu");
      expect(closeOnly.message).toContain("openTime");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("cover photo", () => {
  test("the upload receipt's timestamp crosses as an i64 string", async () => {
    const client = await offlineClient();
    try {
      // Above 2^53: the reason `timestamp` is a string and not a number.
      const large = await rejection(
        client.setBusinessCoverPhoto({
          id: "fbid-1",
          token: "hmac-1",
          timestamp: "9007199254740993",
        })
      );
      expect(large.kind).toBe("not-connected");

      const notAnInteger = await rejection(
        client.setBusinessCoverPhoto({ id: "fbid-1", token: "hmac-1", timestamp: "1.5" })
      );
      expect(notAnInteger.kind).toBe("invalid-argument");
      expect(notAnInteger.message).toContain("timestamp");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("money precision", () => {
  const declarations = readFileSync(
    join(import.meta.dir, "..", "pkg", "whatsapp_rust_bridge.d.ts"),
    "utf8"
  );

  test("a price crosses as a string, not a number", () => {
    const price = declarations.match(/export interface PriceResult \{[^}]*\}/)?.[0];
    expect(price).toBeTruthy();
    expect(price).toContain("amount1000: string");
    expect(price).not.toContain("amount1000: number");
  });

  test("the magnitude at stake really is lost as a number", () => {
    // WhatsApp scales by 1000, so an amount above 2^53 thousandths is reachable
    // and a `number` would not be able to hold it. This is the loss the string
    // above prevents, demonstrated rather than asserted.
    const exact = "9007199254740993";
    expect(String(Number(exact))).not.toBe(exact);
    expect(BigInt(exact).toString()).toBe(exact);
  });

  test("every money-bearing result routes through PriceResult", () => {
    for (const declared of [
      "export interface SalePriceResult",
      "export interface OrderPriceDetailsResult",
    ]) {
      expect(declarations).toContain(declared);
    }

    const details = declarations.match(
      /export interface OrderPriceDetailsResult \{[^}]*\}/
    )?.[0];
    expect(details).toContain("PriceResult");

    const product = declarations.match(/export interface ProductResult \{[^}]*\}/)?.[0];
    expect(product).toContain("PriceResult");
  });
});

describe("emitted types", () => {
  const declarations = readFileSync(
    join(import.meta.dir, "..", "pkg", "whatsapp_rust_bridge.d.ts"),
    "utf8"
  );

  test("every new operation is declared", () => {
    for (const declared of [
      "getCatalog(jid: string, options?: CatalogOptionsInput | null): Promise<CatalogResult>",
      "getCollections(jid: string, options?: CollectionOptionsInput | null): Promise<CollectionsResult>",
      "getOrder(jid: string, order_id: string, token: string): Promise<OrderResult>",
      "updateBusinessProfile(update: BusinessProfileUpdateInput): Promise<void>",
      "setBusinessCoverPhoto(upload: CoverPhotoUploadInput): Promise<void>",
      "removeBusinessCoverPhoto(id: string): Promise<void>",
    ]) {
      expect(declarations).toContain(declared);
    }
  });

  test("the cursor asymmetry is preserved, not smoothed over", () => {
    // The catalog query takes `after`; the response carries both cursors.
    const catalogOptions = declarations.match(
      /export interface CatalogOptionsInput \{[^}]*\}/
    )?.[0];
    expect(catalogOptions).toContain("after?");
    expect(catalogOptions).not.toContain("before?");

    const catalog = declarations.match(/export interface CatalogResult \{[^}]*\}/)?.[0];
    expect(catalog).toContain("afterCursor?");
    expect(catalog).toContain("beforeCursor?");

    // Collections have a forward cursor only, on both sides.
    const collectionOptions = declarations.match(
      /export interface CollectionOptionsInput \{[^}]*\}/
    )?.[0];
    expect(collectionOptions).toContain("after?");
    expect(collectionOptions).not.toContain("before?");

    const collections = declarations.match(
      /export interface CollectionsResult \{[^}]*\}/
    )?.[0];
    expect(collections).toContain("afterCursor?");
    expect(collections).not.toContain("beforeCursor");
  });

  test("no product mutation is exposed", () => {
    // The core verified these appear in no WhatsApp Web bundle. Not exposed,
    // not requested upstream, not implemented here.
    for (const absent of ["productCatalogAdd", "productCatalogEdit", "productCatalogDelete"]) {
      expect(declarations).not.toContain(absent);
    }
  });

  test("a variant order keeps the properties needed to fulfil it", () => {
    const orderProduct = declarations.match(
      /export interface OrderProductResult \{[^}]*\}/
    )?.[0];
    expect(orderProduct).toContain("variantProperties: VariantPropertyResult[]");
  });
});
