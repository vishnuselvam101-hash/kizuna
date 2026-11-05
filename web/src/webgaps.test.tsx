import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AccountView } from "./AccountView";
import { ClaimReview } from "./claims";
import { DecisionsView } from "./DecisionsView";
import { DocumentsView } from "./DocumentsView";
import { artifact, claim } from "./fixtures";
import { I18nProvider } from "./i18n";
import { LoginView } from "./LoginView";
import { axeViolations, fakeApi, ME, renderWithSession } from "./test-utils";
import type { Artifact, MfaStatus } from "./types";
import { assertionJson, b64urlToBuf, bufToB64url, requestOptions } from "./webauthn";

const TOKENS = { access_token: "acc", refresh_token: "ref", mfa_verified: true };
const err = (status: number, error: string) => () => new Response(JSON.stringify({ error }), { status });

function fillFirstFactor() {
  fireEvent.change(screen.getByLabelText("Organisation"), { target: { value: "acme" } });
  fireEvent.change(screen.getByLabelText("Email"), { target: { value: "a@example.test" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw-123456789012" } });
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
}

const buf = (s: string) => new TextEncoder().encode(s).buffer;

function stubKey(result: unknown) {
  const get = vi.fn(async () => result);
  Object.defineProperty(window, "PublicKeyCredential", { value: function () {}, configurable: true });
  Object.defineProperty(navigator, "credentials", { value: { get, create: vi.fn() }, configurable: true });
  return get;
}

test("base64url helpers round-trip and decode server options", () => {
  const raw = new Uint8Array([0, 251, 255, 1, 62, 63]);
  const s = bufToB64url(raw.buffer)!;
  expect(s).not.toMatch(/[+/=]/);
  expect([...new Uint8Array(b64urlToBuf(s))]).toEqual([...raw]);
  const o = requestOptions({ publicKey: { challenge: s, allowCredentials: [{ type: "public-key", id: s }], userVerification: "required" } });
  expect(new Uint8Array(o.publicKey!.challenge as ArrayBuffer)).toEqual(raw);
  const j = assertionJson({
    id: "k",
    rawId: raw.buffer,
    type: "public-key",
    response: { authenticatorData: raw.buffer, clientDataJSON: raw.buffer, signature: raw.buffer, userHandle: null },
  });
  expect(j.response.userHandle).toBeNull();
  expect(j.rawId).toBe(s);
});

test("sign-in with an enrolled security key: password, then navigator.credentials.get, then finish", async () => {
  const get = stubKey({
    id: "cred",
    rawId: buf("cred"),
    type: "public-key",
    response: { authenticatorData: buf("ad"), clientDataJSON: buf("cd"), signature: buf("sig"), userHandle: buf("u") },
    getClientExtensionResults: () => ({}),
  });
  let finish: Record<string, unknown> | null = null;
  const { api } = fakeApi({
    "POST /auth/login": err(401, "webauthn_required"),
    "POST /auth/webauthn/login/start": { challenge_id: "ch-1", public_key: { publicKey: { challenge: "AAEC", allowCredentials: [] } } },
    "POST /auth/webauthn/login/finish": (b: unknown) => ((finish = b as Record<string, unknown>), TOKENS),
  });
  const onLogin = vi.fn();
  const { container } = render(
    <I18nProvider initial="en">
      <LoginView api={api} onLogin={onLogin} />
    </I18nProvider>,
  );
  fillFirstFactor();
  await screen.findByText(/protected by a security key/);
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: "Use security key" }));
  await waitFor(() => expect(onLogin).toHaveBeenCalled());
  expect(get).toHaveBeenCalledTimes(1);
  expect(finish).toMatchObject({ tenant: "acme", challenge_id: "ch-1", credential: { id: "cred", response: { signature: bufToB64url(buf("sig")) } } });
  expect(api.authenticated).toBe(true);
  expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("");
});

test("a cancelled key prompt is explained and nothing is sent", async () => {
  stubKey(null);
  const { api, calls } = fakeApi({
    "POST /auth/login": err(401, "webauthn_required"),
    "POST /auth/webauthn/login/start": { challenge_id: "ch-1", public_key: { publicKey: { challenge: "AAEC" } } },
  });
  render(
    <I18nProvider initial="en">
      <LoginView api={api} onLogin={() => {}} />
    </I18nProvider>,
  );
  fillFirstFactor();
  fireEvent.click(await screen.findByRole("button", { name: "Use security key" }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/did not respond/);
  expect(calls.some((c) => c.path === "/auth/webauthn/login/finish")).toBe(false);
});

test("authenticator code step, and the lost-key recovery path in Japanese", async () => {
  const { api, calls } = fakeApi({
    "POST /auth/login": (b: { totp_code: string | null }) => (b.totp_code ? TOKENS : err(401, "mfa_required")()),
  });
  const onLogin = vi.fn();
  render(
    <I18nProvider initial="en">
      <LoginView api={api} onLogin={onLogin} />
    </I18nProvider>,
  );
  fillFirstFactor();
  fireEvent.change(await screen.findByLabelText("Authenticator code"), { target: { value: "123456" } });
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
  await waitFor(() => expect(onLogin).toHaveBeenCalled());
  expect(calls.at(-1)!.body).toMatchObject({ totp_code: "123456" });

  const r = fakeApi({ "POST /auth/login": err(401, "webauthn_required"), "POST /auth/webauthn/recover": { ...TOKENS, mfa_verified: false } });
  const onLogin2 = vi.fn();
  render(
    <I18nProvider initial="ja">
      <LoginView api={r.api} onLogin={onLogin2} />
    </I18nProvider>,
  );
  const forms = screen.getAllByRole("form");
  const ja = within(forms[forms.length - 1]);
  fireEvent.change(ja.getByLabelText("組織"), { target: { value: "acme" } });
  fireEvent.change(ja.getByLabelText("メールアドレス"), { target: { value: "a@example.test" } });
  fireEvent.change(ja.getByLabelText("パスワード"), { target: { value: "pw-123456789012" } });
  fireEvent.click(ja.getByRole("button", { name: "サインイン" }));
  fireEvent.click(await ja.findByRole("button", { name: "キーを紛失した場合はリカバリーコードを使用" }));
  fireEvent.change(ja.getByLabelText("リカバリーコード"), { target: { value: " ABCD-EFGH " } });
  fireEvent.click(ja.getByRole("button", { name: "リカバリーコードでサインイン" }));
  await screen.findByRole("heading", { name: "リカバリーコードを確認しました" });
  expect(r.calls.at(-1)!.body).toMatchObject({ tenant: "acme", recovery_code: "ABCD-EFGH" });
  expect(onLogin2).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "続ける" }));
  expect(onLogin2).toHaveBeenCalled();
});

const status = (over: Partial<MfaStatus> = {}): MfaStatus => ({
  totp_enabled: false,
  webauthn_enabled: true,
  credentials: [
    { id: "k1", label: "YubiKey desk", created_at: "2026-11-01T00:00:00Z", last_used_at: "2026-11-04T00:00:00Z" },
    { id: "k2", label: "Spare", created_at: "2026-11-02T00:00:00Z", last_used_at: null },
  ],
  recovery_codes_remaining: 2,
  ...over,
});

test("account security: TOTP enrolment with step-up, then verify", async () => {
  let st = status({ webauthn_enabled: false, credentials: [], recovery_codes_remaining: 0 });
  const { api, calls } = fakeApi({
    "GET /auth/mfa/status": () => st,
    "POST /auth/mfa/totp/setup": { secret_base32: "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP", otpauth_uri: "otpauth://totp/KIZUNA:a?secret=JBSWY3DPEHPK3PXP&issuer=KIZUNA" },
    "POST /auth/mfa/totp/confirm": () => ((st = { ...st, totp_enabled: true }), { ok: true }),
  });
  const { container } = renderWithSession(<AccountView onRelogin={() => {}} />, { api });
  const panel = within(await screen.findByRole("region", { name: "Authenticator app (TOTP)" }));
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.change(panel.getByLabelText(/Current password/), { target: { value: "pw-123456789012" } });
  fireEvent.click(panel.getByRole("button", { name: "Set up authenticator app" }));
  await panel.findByText("JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP");
  expect(calls.find((c) => c.path === "/auth/mfa/totp/setup")!.body).toEqual({ password: "pw-123456789012" });
  await panel.findByRole("img", { name: "QR code for the authenticator app" });
  fireEvent.change(panel.getByLabelText("Code shown in the app"), { target: { value: "123456" } });
  fireEvent.click(panel.getByRole("button", { name: "Verify and turn on" }));
  await screen.findByText(/An authenticator app is set up/);
  expect(calls.find((c) => c.path === "/auth/mfa/totp/confirm")!.body).toEqual({ code: "123456" });
});

test("account security: remove a key and regenerate recovery codes (shown once)", async () => {
  const { api, calls } = fakeApi({
    "GET /auth/mfa/status": status(),
    "POST /auth/mfa/webauthn/credentials/k2/remove": { ok: true, remaining: 1 },
    "POST /auth/mfa/recovery-codes": { recovery_codes: ["AAAA-BBBB", "CCCC-DDDD"] },
  });
  const { container } = renderWithSession(<AccountView onRelogin={() => {}} />, { api });
  await screen.findByText("YubiKey desk");
  expect(screen.getByText(/Few codes left/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Remove security key Spare" }));
  const rm = screen.getByRole("form", { name: "Remove security key “Spare”" });
  expect(within(rm).getByText(/other keys keep working/)).toBeTruthy();
  fireEvent.change(within(rm).getByLabelText(/Current password/), { target: { value: "pw-123456789012" } });
  fireEvent.click(within(rm).getByRole("button", { name: "Remove key" }));
  await screen.findByText("Security key “Spare” removed.");
  expect(calls.find((c) => c.path.endsWith("/remove"))!.body).toEqual({ password: "pw-123456789012" });

  const codes = within(screen.getByRole("region", { name: "Recovery codes" }));
  fireEvent.click(codes.getByRole("button", { name: "Generate new codes…" }));
  fireEvent.change(codes.getByLabelText(/Current password/), { target: { value: "pw-123456789012" } });
  fireEvent.click(codes.getByRole("button", { name: "Generate new codes" }));
  await screen.findByText("AAAA-BBBB");
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: "I have stored them" }));
  expect(screen.queryByText("AAAA-BBBB")).toBeNull();
});

test("account security: a session without a key cannot change keys", async () => {
  const { api } = fakeApi({ "GET /auth/mfa/status": status() });
  renderWithSession(<AccountView onRelogin={() => {}} />, { api, me: { ...ME, session_mfa_verified: false } });
  await screen.findByText(/not signed in with a security key/);
  expect((screen.getByRole("button", { name: "Remove security key Spare" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Generate new codes…" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByText("Register a security key")).toBeNull();
});

const aiClaim = claim("c-ai", "S355", {
  extraction_method: "ai:gpt-x-mini+m1",
  review_status: "ai_suggested",
  confidence_bps: 6200,
  provenance: { ai: { model: "gpt-x-mini" } },
  recorded_by: null,
});

test("single-claim review: AI suggestion shows origin, model and confidence; accept needs a reason", async () => {
  let sent: unknown = null;
  const { api } = fakeApi({
    "GET /artifacts/art-1": { artifact: { ...artifact, created_by: "u-other" }, captures: [{ uploaded_by: "u-other", uploaded_at: "2026-11-01T00:00:00Z", original_filename: "a.pdf", declared_type: null }], claim_count: 1 },
    "POST /claims/c-ai/review": (b: unknown) => ((sent = b), { ...aiClaim, review_status: "accepted" }),
  });
  const onReviewed = vi.fn();
  const { container } = renderWithSession(<ClaimReview claim={aiClaim} onReviewed={onReviewed} />, { api });
  expect(screen.getAllByText("AI suggestion").length).toBeGreaterThan(0);
  expect(screen.getByText("gpt-x-mini")).toBeTruthy();
  expect(screen.getByText(/^62% method confidence/)).toBeTruthy();
  fireEvent.click(await screen.findByRole("button", { name: "Accept suggestion" }));
  expect(sent).toBeNull();
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "Matches drawing rev C" } });
  fireEvent.click(screen.getByRole("button", { name: "Accept suggestion" }));
  await screen.findByText("Review recorded. Status: Accepted.");
  expect(sent).toEqual({ decision: "approve", comment: "Matches drawing rev C" });
  expect(onReviewed).toHaveBeenCalled();
  expect(await axeViolations(container)).toEqual([]);
});

test("single-claim review: four-eyes blocks the uploader; rejected claims are final", async () => {
  const { api, calls } = fakeApi({
    "GET /artifacts/art-1": { artifact: { ...artifact, created_by: ME.user_id }, captures: [], claim_count: 1 },
  });
  const { unmount } = renderWithSession(<ClaimReview claim={aiClaim} onReviewed={() => {}} />, { api });
  await screen.findByText(/another reviewer must decide/);
  expect(screen.queryByRole("button", { name: "Accept suggestion" })).toBeNull();
  unmount();
  renderWithSession(<ClaimReview claim={claim("c-r", "S1", { review_status: "rejected", artifact_id: null })} onReviewed={() => {}} />, { api });
  expect(screen.getByText(/Rejected claims are final/)).toBeTruthy();
  expect(calls.every((c) => c.method === "GET")).toBe(true);
});

test("decision form picks evidence from a searchable claim list", async () => {
  let posted: { evidence: { claim_id: string }[] } | null = null;
  const { api } = fakeApi({
    "GET /decisions": [],
    "GET /claims": [claim("c-1111aaaa", "S355"), claim("c-2222bbbb", "40", { predicate: "mfg.hardness" })],
    "POST /decisions": (b: unknown) => ((posted = b as typeof posted), { id: "d1" }),
  });
  renderWithSession(<DecisionsView />, { api });
  fireEvent.click(screen.getByText("Record a decision"));
  expect(screen.getByText(/Enter the component first/)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Release BRK-100" } });
  fireEvent.change(screen.getByLabelText("Component"), { target: { value: "BRK-100" } });
  fireEvent.change(screen.getByLabelText("Rationale"), { target: { value: "All checks pass" } });
  await screen.findByRole("checkbox", { name: /mfg.hardness/ });
  fireEvent.change(screen.getByLabelText(/Filter claims/), { target: { value: "hard" } });
  expect(screen.queryByRole("checkbox", { name: /mfg.material/ })).toBeNull();
  fireEvent.click(screen.getByRole("checkbox", { name: /mfg.hardness/ }));
  expect(screen.getByText("1 selected")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Record decision" }));
  await waitFor(() => expect(posted).not.toBeNull());
  expect(posted!.evidence).toEqual([{ claim_id: "c-2222bbbb" }]);
});

const quarantined: Artifact = { ...artifact, id: "art-q", status: "quarantined", display_name: "evil.pdf", created_by: "u-up", scan_result: { findings: ["pdf_javascript"] } };

test("quarantine release: four-eyes for the uploader, reason + confirmation for others", async () => {
  let body: unknown = null;
  const detail = (by: string) => ({ artifact: { ...quarantined, created_by: by }, captures: [{ uploaded_by: by, uploaded_at: "2026-11-01T00:00:00Z", original_filename: "evil.pdf", declared_type: null }], claim_count: 0 });
  const own = fakeApi({ "GET /artifacts": [quarantined], "GET /artifacts/art-q": detail(ME.user_id) });
  const first = renderWithSession(<DocumentsView />, { api: own.api });
  fireEvent.click(await screen.findByRole("button", { name: "Review quarantine release of evil.pdf" }));
  await screen.findByText(/another administrator must release it/);
  expect(screen.queryByRole("button", { name: "Release from quarantine" })).toBeNull();
  first.unmount();

  const other = fakeApi({
    "GET /artifacts": [quarantined],
    "GET /artifacts/art-q": detail("u-up"),
    "POST /artifacts/art-q/release": (b: unknown) => ((body = b), { ...quarantined, status: "accepted" }),
  });
  const { container } = renderWithSession(<DocumentsView />, { api: other.api });
  fireEvent.click(await screen.findByRole("button", { name: "Review quarantine release of evil.pdf" }));
  const btn = (await screen.findByRole("button", { name: "Release from quarantine" })) as HTMLButtonElement;
  expect(screen.getByText("pdf_javascript")).toBeTruthy();
  expect(btn.disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("Reason for release"), { target: { value: "False positive: form field script" } });
  expect(btn.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: /checked the scan findings/ }));
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(btn);
  await screen.findByText("“evil.pdf” released from quarantine.");
  expect(body).toEqual({ reason: "False positive: form field script" });
});
