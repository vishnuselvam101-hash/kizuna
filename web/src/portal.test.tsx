import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { PortalApp, type PortalRequest, type PortalUpload } from "./PortalApp";
import { SupplierPortalPanel, type SupplierUser } from "./SupplierPortalPanel";
import { axeViolations, fakeApi, renderWithSession } from "./test-utils";

const req: PortalRequest = {
  id: "r-1",
  reference: "RR-0193A7C2D4E5",
  due_on: "2026-11-19",
  documents: [
    {
      name: "acme-mill-cert.pdf",
      items: [{ part: "BRK-100", what_ja: "材質", what_en: "material grade", purchase_order: "PO-1", po_line: "1", lot: "L-7", heat: null }],
    },
  ],
};
const detail: PortalRequest = { ...req, body_ja: "再提出のお願い", body_en: "Please resend the mill certificate." };
const uploads: PortalUpload[] = [
  { id: "u-1", original_filename: "cert.pdf", size_bytes: 2048, status: "accepted_for_review", created_at: "2026-11-05T10:00:00Z" },
  { id: "u-2", original_filename: "macro.pdf", size_bytes: 900, status: "quarantined", created_at: "2026-11-05T10:01:00Z" },
];

function signedIn() {
  const f = fakeApi({
    "GET /portal/requests": [req],
    "GET /portal/requests/r-1": detail,
    "GET /portal/requests/r-1/uploads": uploads,
    "POST /portal/requests/r-1/uploads": { ...uploads[0], id: "u-3", original_filename: "new.pdf", status: "accepted_for_review" },
    "POST /portal/logout": { ok: true },
  });
  f.api.setTokens({ access_token: "kzp.x.y", refresh_token: "" });
  return f;
}

test.each(["en", "ja"] as const)("portal sign-in has no axe violations and requires the authenticator code (%s)", async (lang) => {
  const { api, calls } = fakeApi({ "POST /portal/login": { access_token: "kzp.t.s", token_type: "Bearer", expires_in: 1 }, "GET /portal/requests": [] });
  const { container } = render(<PortalApp api={api} lang={lang} />);
  expect(await axeViolations(container)).toEqual([]);
  const code = screen.getByLabelText(lang === "en" ? "Authenticator code" : "認証コード") as HTMLInputElement;
  expect(code.required).toBe(true);
  fireEvent.change(screen.getByLabelText(lang === "en" ? "Customer ID" : "取引先ID"), { target: { value: "acme" } });
  fireEvent.change(screen.getByLabelText(lang === "en" ? "Email" : "メールアドレス"), { target: { value: "s@x.test" } });
  fireEvent.change(screen.getByLabelText(lang === "en" ? "Password" : "パスワード"), { target: { value: "correct horse battery" } });
  fireEvent.change(code, { target: { value: "123456" } });
  fireEvent.submit(code.closest("form")!);
  await screen.findByText(lang === "en" ? "There are no open requests for your company." : "貴社宛ての未回答の依頼はありません。");
  expect(calls.find((c) => c.path === "/portal/login")?.body).toMatchObject({ tenant: "acme", totp_code: "123456" });
});

test.each(["en", "ja"] as const)("request list and detail have no axe violations (%s)", async (lang) => {
  const { api } = signedIn();
  const { container } = render(<PortalApp api={api} lang={lang} />);
  await screen.findByText("RR-0193A7C2D4E5");
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: lang === "en" ? "Open request RR-0193A7C2D4E5" : "依頼 RR-0193A7C2D4E5 を開く" }));
  await screen.findByText(lang === "en" ? "Accepted for review" : "確認待ちとして受理");
  expect(screen.getByText(lang === "en" ? "Held for security check" : "安全確認のため保留")).toBeTruthy();
  expect(screen.getByText(lang === "en" ? /material grade · part BRK-100/ : /材質 · 品番 BRK-100/)).toBeTruthy();
  expect(await axeViolations(container)).toEqual([]);
});

test("upload posts the file to the request and refreshes the status list", async () => {
  const { api, calls } = signedIn();
  render(<PortalApp api={api} lang="en" />);
  fireEvent.click(await screen.findByRole("button", { name: "Open request RR-0193A7C2D4E5" }));
  const input = (await screen.findByLabelText("File (PDF, image or spreadsheet)")) as HTMLInputElement;
  const file = new File(["%PDF-1.7"], "new.pdf", { type: "application/pdf" });
  fireEvent.change(input, { target: { files: [file] } });
  fireEvent.submit(input.closest("form")!);
  await screen.findByText(/Received new.pdf/);
  const post = calls.find((c) => c.method === "POST" && c.path === "/portal/requests/r-1/uploads")!;
  expect(post.body).toBeInstanceOf(FormData);
  expect(calls.filter((c) => c.path === "/portal/requests/r-1/uploads" && c.method === "GET").length).toBeGreaterThan(1);
  // The portal never calls the tenant API.
  expect(calls.every((c) => c.path.startsWith("/portal/"))).toBe(true);
});

test("invitation: password, then authenticator enrolment, then signed in; the link token leaves the address bar", async () => {
  window.history.replaceState(null, "", "/portal#invite=kzi.0193a7c2d4e5f6a7b8c9d0e1f2a3b4c5.abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG");
  const { api, calls } = fakeApi({
    "POST /portal/invite/start": { email: "s@x.test", display_name: "Sato", supplier: "Acme Steel", secret_base32: "JBSWY3DPEHPK3PXP", otpauth_uri: "otpauth://totp/x" },
    "POST /portal/invite/finish": { access_token: "kzp.t.s" },
    "GET /portal/requests": [],
  });
  const { container } = render(<PortalApp api={api} lang="en" />);
  expect(window.location.hash).toBe("");
  expect(await axeViolations(container)).toEqual([]);
  const pw = screen.getByLabelText("New password");
  fireEvent.change(pw, { target: { value: "a long enough password" } });
  fireEvent.submit(pw.closest("form")!);
  const code = await screen.findByLabelText("Authenticator code");
  expect(screen.getByText("JBSWY3DPEHPK3PXP")).toBeTruthy();
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.change(code, { target: { value: "654321" } });
  fireEvent.submit(code.closest("form")!);
  await screen.findByText("There are no open requests for your company.");
  expect(calls.map((c) => c.path)).toEqual(["/portal/invite/start", "/portal/invite/finish", "/portal/requests"]);
});

test("an invalid invitation explains what to do", async () => {
  window.history.replaceState(null, "", "/portal#invite=kzi.bad.tokenvalue");
  const { api } = fakeApi({ "POST /portal/invite/start": () => new Response(JSON.stringify({ error: "invalid_invite" }), { status: 401 }) });
  render(<PortalApp api={api} lang="ja" />);
  const pw = screen.getByLabelText("新しいパスワード");
  fireEvent.change(pw, { target: { value: "a long enough password" } });
  fireEvent.submit(pw.closest("form")!);
  expect((await screen.findByRole("alert")).textContent).toMatch(/招待リンクは無効/);
});

const su = (o: Partial<SupplierUser>): SupplierUser => ({
  id: "su-1",
  supplier_id: "s-1",
  supplier: "Acme Steel",
  email: "sato@acme.test",
  display_name: "Sato",
  status: "active",
  invited_at: "2026-11-01T00:00:00Z",
  invite_expires_at: "2026-11-04T00:00:00Z",
  activated_at: "2026-11-01T01:00:00Z",
  revoked_at: null,
  ...o,
});

test.each(["en", "ja"] as const)("admin supplier-user panel: invite shows the link once, revoke asks first, no axe violations (%s)", async (lang) => {
  const { api, calls } = fakeApi({
    "GET /portal-users": [su({}), su({ id: "su-2", email: "new@b.test", status: "invited" })],
    "POST /portal-users": { user: su({ id: "su-3", email: "kim@b.test", status: "invited" }), invite_token: "kzi.t.secret" },
    "POST /portal-users/su-1/revoke": su({ status: "revoked" }),
  });
  const { container } = renderWithSession(<SupplierPortalPanel />, { api, lang });
  await screen.findByText("sato@acme.test");
  expect(await axeViolations(container)).toEqual([]);
  const en = lang === "en";
  fireEvent.change(screen.getByLabelText(en ? "Supplier" : "サプライヤー"), { target: { value: "Borealis" } });
  fireEvent.change(screen.getByLabelText(en ? "Contact name" : "担当者名"), { target: { value: "Kim" } });
  fireEvent.change(screen.getByLabelText(en ? "Contact email" : "担当者メールアドレス"), { target: { value: "kim@b.test" } });
  fireEvent.click(screen.getByRole("button", { name: en ? "Create invitation" : "招待を作成" }));
  expect((await screen.findByText(/\/portal#invite=kzi\.t\.secret$/)).tagName).toBe("CODE");
  expect(calls.find((c) => c.method === "POST" && c.path === "/portal-users")?.body).toEqual({ supplier: "Borealis", email: "kim@b.test", display_name: "Kim" });
  fireEvent.click(screen.getByRole("button", { name: en ? "Revoke portal access of sato@acme.test" : "sato@acme.test のポータル利用権限を取り消す" }));
  expect(calls.some((c) => c.path.endsWith("/revoke"))).toBe(false);
  expect(await axeViolations(container)).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: en ? "Confirm" : "確定" }));
  await waitFor(() => expect(calls.some((c) => c.path === "/portal-users/su-1/revoke")).toBe(true));
});

test("a viewer sees supplier users but cannot invite or revoke", async () => {
  const { api } = fakeApi({ "GET /portal-users": [su({})] });
  renderWithSession(<SupplierPortalPanel />, { api, me: { user_id: "v", tenant_id: "t1", email: "v@x.test", display_name: "V", role: "auditor", mfa_enabled: true, session_mfa_verified: true } });
  await screen.findByText("sato@acme.test");
  expect(screen.queryByRole("button", { name: /Revoke/ })).toBeNull();
  expect(screen.queryByRole("button", { name: "Create invitation" })).toBeNull();
});


