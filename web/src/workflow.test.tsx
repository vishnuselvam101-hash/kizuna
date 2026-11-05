import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { App } from "./App";
import { DecisionsView } from "./DecisionsView";
import { DocumentsView } from "./DocumentsView";
import { artifact, claim, conflict, conflictDetail, decision, trace, users } from "./fixtures";
import { ReportsView } from "./ReportsView";
import { UsersView } from "./UsersView";
import { fakeApi, ME, renderWithSession, type Handler } from "./test-utils";

const TOKENS = { access_token: "acc-SECRET", refresh_token: "ref-SECRET", mfa_verified: true };

function backend(extra: Record<string, Handler | unknown> = {}) {
  return fakeApi({
    "POST /auth/login": TOKENS,
    "POST /auth/logout": { ok: true },
    "GET /auth/me": ME,
    "GET /artifacts": [artifact],
    "GET /claims": [claim("c-a", "S355")],
    "GET /conflicts?status=open": [conflict],
    "GET /conflicts?status=needs_evidence": [],
    "GET /decisions": [decision],
    "GET /audit/verify": { intact: true, events_checked: 42, problem: null, head_seq: 42, head_hash: "aa".repeat(32) },
    "GET /conflicts/cf-1": conflictDetail,
    ...extra,
  });
}

async function signIn() {
  fireEvent.change(screen.getByLabelText("Organisation"), { target: { value: "acme" } });
  fireEvent.change(screen.getByLabelText("Email"), { target: { value: "admin@example.test" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "correct horse battery" } });
  fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
  await screen.findByRole("heading", { name: "Overview", level: 1 });
}

function storageEmpty() {
  expect(localStorage.length).toBe(0);
  expect(sessionStorage.length).toBe(0);
  expect(document.cookie).toBe("");
}

test("sign in → dashboard → conflict review → review recorded → sign out clears everything", async () => {
  let reviewed: unknown = null;
  const { api, calls } = backend({
    "POST /conflicts/cf-1/review": (b: unknown) => ((reviewed = b), { ok: true }),
  });
  const { container } = render(<App api={api} lang="en" />);
  await signIn();
  expect(screen.getByText("Aiko Admin · Administrator")).toBeTruthy();
  const kpis = screen.getByRole("list", { name: "Key figures" });
  expect(within(kpis).getByText("Open conflicts").closest("button")!.textContent).toContain("1");
  expect(within(kpis).getByText("Intact")).toBeTruthy();
  // Bearer token sent from memory; never persisted.
  expect(calls.find((c) => c.path === "/auth/me")!.headers.authorization).toBe("Bearer acc-SECRET");
  storageEmpty();

  fireEvent.click(within(screen.getByRole("navigation", { name: "Primary" })).getByRole("button", { name: "Conflicts" }));
  fireEvent.click(await screen.findByRole("button", { name: "BRK-100" }));
  await screen.findByRole("heading", { name: "Contradicting source" });
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "PO superseded by rev D" } });
  fireEvent.click(screen.getByRole("button", { name: "Reject evidence" }));
  await screen.findByText("Review recorded in the audit trail.");
  expect(reviewed).toEqual({ decision: "reject", comment: "PO superseded by rev D" });

  fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
  await screen.findByRole("heading", { name: "Sign in" });
  expect(api.authenticated).toBe(false);
  expect(container.textContent).not.toContain("BRK-100");
  expect(container.textContent).not.toContain("Aiko Admin");
  expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("");
  expect(container.innerHTML).not.toContain("SECRET");
  storageEmpty();
});

test("an expired session that cannot refresh returns to sign-in and drops tenant data", async () => {
  let expired = false;
  const { api } = backend({
    "GET /decisions": () => (expired ? new Response(JSON.stringify({ error: "invalid_token" }), { status: 401 }) : [decision]),
    "POST /auth/refresh": new Response(JSON.stringify({ error: "invalid_token" }), { status: 401 }),
  });
  const { container } = render(<App api={api} lang="en" />);
  await signIn();
  expired = true;
  fireEvent.click(within(screen.getByRole("navigation", { name: "Primary" })).getByRole("button", { name: "Decisions" }));
  await screen.findByRole("heading", { name: "Sign in" });
  expect(api.authenticated).toBe(false);
  expect(container.textContent).not.toContain("Release BRK-100");
  storageEmpty();
});

test("viewer does not see admin navigation or write actions", async () => {
  const { api } = backend({ "GET /auth/me": { ...ME, role: "viewer", display_name: "Vic Viewer" } });
  render(<App api={api} lang="en" />);
  await signIn();
  const nav = screen.getByRole("navigation", { name: "Primary" });
  expect(within(nav).queryByRole("button", { name: "Admin" })).toBeNull();
  fireEvent.click(within(nav).getByRole("button", { name: "Evidence" }));
  await screen.findByText("BRK-100 spec rev C.pdf");
  expect(screen.queryByRole("button", { name: "Upload" })).toBeNull();
  expect(screen.queryByRole("button", { name: /Extract claims/ })).toBeNull();
});

test("command palette navigates with the keyboard", async () => {
  const { api } = backend({ "GET /users": users });
  render(<App api={api} lang="en" />);
  await signIn();
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  const input = await screen.findByRole("combobox", { name: "Command palette" });
  fireEvent.change(input, { target: { value: "users" } });
  fireEvent.keyDown(input, { key: "Enter" });
  await screen.findByRole("heading", { name: "Users & access", level: 1 });
});

test("document upload sends multipart kind + file and reports the stored status", async () => {
  let sent: FormData | null = null;
  const { api } = fakeApi({
    "GET /artifacts": [],
    "POST /artifacts": (b: unknown) => ((sent = b as FormData), { artifact: { ...artifact, status: "accepted" }, duplicate: false }),
  });
  renderWithSession(<DocumentsView />, { api });
  await screen.findByText(/No documents yet/);
  fireEvent.change(screen.getByLabelText("Document kind"), { target: { value: "drawing" } });
  const file = new File(["%PDF-1.7"], "drw.pdf", { type: "application/pdf" });
  fireEvent.change(screen.getByLabelText("File"), { target: { files: [file] } });
  fireEvent.click(screen.getByRole("button", { name: "Upload" }));
  await screen.findByText("Stored. Status: Accepted.");
  expect(sent!.get("kind")).toBe("drawing");
  expect((sent!.get("file") as File).name).toBe("drw.pdf");
});

test("decision trace: revoke needs explicit confirmation; reconfirm posts the comment", async () => {
  const posted: string[] = [];
  const { api } = fakeApi({
    "GET /decisions": [decision],
    "GET /decisions/d-1/trace": trace,
    "POST /decisions/d-1/revoke": () => (posted.push("revoke"), decision),
    "POST /decisions/d-1/reconfirm": (b: unknown) => (posted.push(`reconfirm:${(b as { comment: string }).comment}`), decision),
  });
  renderWithSession(<DecisionsView />, { api });
  fireEvent.click(await screen.findByRole("button", { name: "Release BRK-100 from Supplier A" }));
  await screen.findByText("Evidence changed. A reviewer must reconfirm or revoke this decision.");
  expect(screen.getByText("material claim superseded")).toBeTruthy();
  expect(screen.getByRole("group", { name: /Trace graph: 2 evidence items/ })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Revoke…" }));
  expect(posted).toEqual([]);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "S275 accepted by design" } });
  fireEvent.click(screen.getByRole("button", { name: "Reconfirm with current evidence" }));
  await waitFor(() => expect(posted).toEqual(["reconfirm:S275 accepted by design"]));
});

test("users: disabling asks for confirmation before PATCH", async () => {
  const patches: unknown[] = [];
  const { api } = fakeApi({ "GET /users": users, "PATCH /users/u-2": (b: unknown) => (patches.push(b), users[1]) });
  renderWithSession(<UsersView />, { api });
  fireEvent.click(await screen.findByRole("button", { name: "Disable rev@example.test" }));
  expect(patches).toEqual([]);
  expect(screen.getByRole("alertdialog").textContent).toMatch(/Disable rev@example.test\?/);
  fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
  await waitFor(() => expect(patches).toEqual([{ status: "disabled" }]));
  // own row is not editable (no self-lockout)
  expect(screen.queryByRole("button", { name: "Disable admin@example.test" })).toBeNull();
});

test("audit report: markdown report is generated for the chosen decision and offered for download", async () => {
  const { api, calls } = fakeApi({
    "GET /decisions": [decision],
    "GET /decisions/d-1/report?format=markdown": "# Verification report\nsha256: abc",
    "GET /audit/events?limit=200": [{ seq: 1, occurred_at: "2026-11-01T00:00:00Z", actor_user_id: "u-admin", actor_kind: "user", action: "user.created", target_type: "user", target_id: "u-2", details: {}, hash: "ff".repeat(32) }],
    "GET /audit/verify": { intact: true, events_checked: 1, problem: null, head_seq: 1, head_hash: "ff".repeat(32) },
  });
  const created: Blob[] = [];
  URL.createObjectURL = ((b: Blob) => (created.push(b), "blob:x")) as typeof URL.createObjectURL;
  URL.revokeObjectURL = () => {};
  renderWithSession(<ReportsView />, { api });
  await screen.findByText("user.created");
  await act(async () => {
    fireEvent.change(screen.getByLabelText("Decision"), { target: { value: "d-1" } });
  });
  fireEvent.click(screen.getByRole("button", { name: "Generate" }));
  await screen.findByText(/# Verification report/);
  fireEvent.click(screen.getByRole("button", { name: "Download" }));
  expect(created).toHaveLength(1);
  expect(await created[0].text()).toContain("Verification report");
  fireEvent.click(screen.getByRole("button", { name: "Verify hash chain" }));
  await screen.findByText(/Hash chain intact: 1 events checked/);
  expect(calls.some((c) => c.path === "/decisions/d-1/report?format=markdown")).toBe(true);
});
