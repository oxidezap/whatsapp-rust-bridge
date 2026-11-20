import { expect, test } from "bun:test";
import { createHttp } from "./helpers";

test("the fetch adapter rejects redirects without reaching their destination", async () => {
  let destinationRequests = 0;
  let sourceRequests = 0;
  const destination = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch() {
    destinationRequests++;
    return new Response("must not be reached");
  } });
  const source = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch() {
    sourceRequests++;
    return Response.redirect(destination.url.toString(), 302);
  } });
  try {
    await expect(createHttp().execute(source.url.toString(), "GET", {}, null)).rejects.toThrow();
    expect(sourceRequests).toBe(1);
    expect(destinationRequests).toBe(0);
  } finally {
    source.stop(true);
    destination.stop(true);
  }
});
