import { Api, ApiError } from "./api";

function res(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

test("login stores tokens in memory and sends bearer", async () => {
  const calls: RequestInit[] = [];
  const api = new Api("/api/v1", async (_u, init) => {
    calls.push(init!);
    return calls.length === 1 ? res(200, { access_token: "a1", refresh_token: "r1", mfa_verified: false }) : res(200, { ok: 1 });
  });
  await api.login("t", "e", "p");
  await api.request("GET", "/auth/me");
  expect((calls[1].headers as Record<string, string>).authorization).toBe("Bearer a1");
  expect(globalThis.localStorage.length).toBe(0);
});

test("401 triggers one refresh then retries; failed refresh logs out", async () => {
  const seq = [res(401, { error: "invalid_token" }), res(200, { access_token: "a2", refresh_token: "r2" }), res(200, { ok: 1 })];
  const api = new Api("/api/v1", async () => seq.shift()!);
  api.setTokens({ access_token: "a1", refresh_token: "r1" });
  await expect(api.request("GET", "/auth/me")).resolves.toEqual({ ok: 1 });

  const api2 = new Api("/api/v1", async () => res(401, { error: "invalid_token" }));
  api2.setTokens({ access_token: "a1", refresh_token: "r1" });
  await expect(api2.request("GET", "/auth/me")).rejects.toBeInstanceOf(ApiError);
  expect(api2.authenticated).toBe(false);
});
