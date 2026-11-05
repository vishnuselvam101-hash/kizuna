import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Api } from "./api";
import { EvidenceTable } from "./EvidenceView";
import { LoginView } from "./LoginView";
import type { Claim } from "./types";

const claim = (id: string, v: string): Claim => ({
  id,
  subject_type: "part",
  subject_key: "BRK-7",
  predicate: "mfg.material",
  value_text: v,
  value_num: null,
  value_unit: null,
  revision: "C",
  artifact_id: null,
  source_location: { page: 2 },
  extraction_method: "manual",
  confidence_bps: 10000,
  review_status: "unreviewed",
  recorded_at: "2026-11-05T00:00:00Z",
  superseded_at: null,
});

test("evidence table shows both conflicting claims, source and review status", () => {
  render(<EvidenceTable groups={[{ predicate: "mfg.material", current: [claim("a", "S355"), claim("b", "S275")], superseded_count: 1, disagreement: true }]} />);
  expect(screen.getByText("S355")).toBeTruthy();
  expect(screen.getByText("S275")).toBeTruthy();
  expect(screen.getByText(/sources disagree/)).toBeTruthy();
  expect(screen.getAllByText("unreviewed").length).toBe(2);
  expect(screen.getAllByText(/page 2/).length).toBe(2);
});

test("login asks for TOTP when the server requires MFA and clears secrets", async () => {
  const api = new Api("/api/v1", async () => new Response(JSON.stringify({ error: "mfa_required" }), { status: 401 }));
  render(<LoginView api={api} onLogin={() => {}} />);
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
  fireEvent.click(screen.getByText("Sign in"));
  await waitFor(() => expect(screen.getByLabelText("Authenticator code")).toBeTruthy());
});
