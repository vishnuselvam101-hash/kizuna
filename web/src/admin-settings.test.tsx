import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { vi } from "vitest";
import { AiExtractionPanel, type AiExtraction } from "./AiExtractionPanel";
import { DetectionSettingsPanel, validateDetection } from "./DetectionSettingsPanel";
import { AI_NOTICE_TEXT, aiExtraction, detectionSettings } from "./fixtures";
import { fakeApi, ME, renderWithSession } from "./test-utils";

const fail = (status: number, error: string) => new Response(JSON.stringify({ error }), { status });

function aiBackend(initial: AiExtraction) {
  let state = initial;
  const fake = fakeApi({
    "GET /integrations/ai-extraction": () => state,
    "PUT /integrations/ai-extraction": (b: unknown) => {
      const body = b as { expected_version: number; enabled: boolean; accept_notice?: string };
      if (body.expected_version !== state.version) return fail(409, "settings_version_conflict");
      if (body.enabled && body.accept_notice !== state.notice.version) return fail(400, "notice_not_accepted");
      state = {
        ...state,
        version: state.version + 1,
        settings: { enabled: body.enabled, notice_version: body.enabled ? state.notice.version : state.settings.notice_version },
        active: body.enabled && state.provider.configured,
      };
      return { version: state.version };
    },
  });
  return { ...fake, setState: (s: AiExtraction) => (state = s) };
}

const puts = (calls: { method: string; body: unknown }[]) => calls.filter((c) => c.method === "PUT").map((c) => c.body);

test("AI extraction: notice shown verbatim; enabling needs the explicit acknowledgement", async () => {
  const { api, calls } = aiBackend(aiExtraction());
  renderWithSession(<AiExtractionPanel />, { api, lang: "ja" });
  const notice = await screen.findByTestId("ai-notice");
  expect(notice.textContent).toBe(AI_NOTICE_TEXT);
  expect(screen.getByText("オフ")).toBeTruthy();
  expect(screen.getByText("extract-small-1")).toBeTruthy();
  const on = screen.getByRole("button", { name: "AI 支援抽出を有効にする" }) as HTMLButtonElement;
  expect(on.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox"));
  expect(on.disabled).toBe(false);
  fireEvent.click(on);
  await screen.findByText("オン");
  expect(puts(calls)).toEqual([{ expected_version: 3, enabled: true, accept_notice: "2026-11-01" }]);
});

test("AI extraction: the off switch turns it off without a notice", async () => {
  const { api, calls } = aiBackend(aiExtraction({ settings: { enabled: true, notice_version: "2026-11-01" }, active: true }));
  renderWithSession(<AiExtractionPanel />, { api });
  await screen.findByText("On");
  expect(screen.queryByRole("checkbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Turn off AI-assisted extraction" }));
  await screen.findByText("AI-assisted extraction turned off (version 4).");
  expect(screen.getByText("Off")).toBeTruthy();
  expect(puts(calls)).toEqual([{ expected_version: 3, enabled: false }]);
});

test("AI extraction: a changed notice must be accepted again", async () => {
  const { api } = aiBackend(aiExtraction({ settings: { enabled: true, notice_version: "2026-01-01" }, active: false }));
  renderWithSession(<AiExtractionPanel />, { api });
  await screen.findByText(/must be accepted again/);
  expect(screen.getByRole("button", { name: "Turn off AI-assisted extraction" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "Accept notice and keep on" }) as HTMLButtonElement).disabled).toBe(true);
});

test("AI extraction: provider not configured by the operator cannot be enabled", async () => {
  const { api } = aiBackend(aiExtraction({ provider: { configured: false } }));
  renderWithSession(<AiExtractionPanel />, { api });
  await screen.findByText(/Not configured by the operator/);
  expect(screen.queryByText("Model")).toBeNull();
  expect((screen.getByRole("checkbox") as HTMLInputElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Turn on AI-assisted extraction" }) as HTMLButtonElement).disabled).toBe(true);
});

test("AI extraction: a session without MFA is told to sign in again and cannot change it", async () => {
  const { api } = aiBackend(aiExtraction());
  renderWithSession(<AiExtractionPanel />, { api, me: { ...ME, session_mfa_verified: false } });
  await screen.findByText(/sign in again with your code/);
  expect((screen.getByRole("checkbox") as HTMLInputElement).disabled).toBe(true);
});

test("AI extraction: server errors are explained; a version conflict reloads the latest settings", async () => {
  const b = aiBackend(aiExtraction());
  renderWithSession(<AiExtractionPanel />, { api: b.api });
  await screen.findByTestId("ai-notice");
  b.setState(aiExtraction({ version: 5, notice: { version: "2026-11-02", text: "New notice." } }));
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Turn on AI-assisted extraction" }));
  expect((await screen.findByRole("alert")).textContent).toMatch(/Someone else changed these settings/);
  await screen.findByText("New notice.");
  fireEvent.click(screen.getByRole("checkbox"));
  fireEvent.click(screen.getByRole("button", { name: "Turn on AI-assisted extraction" }));
  await screen.findByText("On");
  expect(puts(b.calls).at(-1)).toEqual({ expected_version: 5, enabled: true, accept_notice: "2026-11-02" });

  const { api } = fakeApi({ "GET /integrations/ai-extraction": aiExtraction(), "PUT /integrations/ai-extraction": () => fail(403, "mfa_required") });
  renderWithSession(<AiExtractionPanel />, { api });
  const boxes = await screen.findAllByRole("checkbox");
  fireEvent.click(boxes.at(-1)!);
  fireEvent.click(screen.getAllByRole("button", { name: "Turn on AI-assisted extraction" }).at(-1)!);
  await screen.findByText(/needs a session verified with your authenticator/);
});

function detBackend(over = {}) {
  return fakeApi({
    "GET /security/detection-settings": detectionSettings(over),
    "PUT /security/detection-settings": { version: 3 },
  });
}

test("detection: thresholds show bounds, defaults and active values; invalid input is not sent", async () => {
  const { api, calls } = detBackend();
  renderWithSession(<DetectionSettingsPanel />, { api });
  const lock = (await screen.findByLabelText("Threshold: Failed sign-ins before lock")) as HTMLInputElement;
  expect(screen.getAllByText(/3–100 events · default 10/).length).toBe(1);
  const row = screen.getByRole("row", { name: /Uploads/ });
  expect(within(row).getByText("180")).toBeTruthy();
  fireEvent.change(lock, { target: { value: "4" } });
  fireEvent.click(screen.getByRole("button", { name: "Save thresholds" }));
  expect((await screen.findByRole("alert")).textContent).toBe("1 values are out of range.");
  expect(lock.getAttribute("aria-invalid")).toBe("true");
  fireEvent.change(lock, { target: { value: "10" } });
  fireEvent.change(screen.getByLabelText("Threshold: Exports"), { target: { value: "5000" } });
  fireEvent.click(screen.getByRole("button", { name: "Save thresholds" }));
  expect(screen.getByLabelText("Threshold: Exports").getAttribute("aria-invalid")).toBe("true");
  expect(puts(calls)).toEqual([]);
});

test("detection: adaptive mode with factor and baseline is saved with the expected version", async () => {
  const { api, calls } = detBackend();
  renderWithSession(<DetectionSettingsPanel />, { api });
  fireEvent.click(await screen.findByRole("checkbox", { name: "Adaptive thresholds" }));
  expect(screen.getByText(/only raises volume rules/)).toBeTruthy();
  fireEvent.change(screen.getByLabelText("Maximum factor"), { target: { value: "9" } });
  fireEvent.click(screen.getByRole("button", { name: "Save thresholds" }));
  await screen.findByRole("alert");
  fireEvent.change(screen.getByLabelText("Maximum factor"), { target: { value: "2.5" } });
  fireEvent.change(screen.getByLabelText("Baseline (days)"), { target: { value: "30" } });
  fireEvent.change(screen.getByLabelText("Threshold: Failed sign-ins per account"), { target: { value: "8" } });
  fireEvent.click(screen.getByRole("button", { name: "Save thresholds" }));
  await screen.findByText("Saved as version 3.");
  const body = puts(calls)[0] as { expected_version: number; settings: { mode: string; adaptive_max_factor: number; baseline_days: number; thresholds: Record<string, number> } };
  expect(body.expected_version).toBe(2);
  expect(body.settings).toMatchObject({ mode: "adaptive", adaptive_max_factor: 2.5, baseline_days: 30 });
  expect(body.settings.thresholds.brute_force).toBe(8);
});

test("detection: reset restores defaults; server rejection is shown", async () => {
  const { api } = fakeApi({
    "GET /security/detection-settings": detectionSettings({ effective: { ...detectionSettings().effective, mode: "adaptive", thresholds: { ...detectionSettings().effective.thresholds, exports: 99 } } }),
    "PUT /security/detection-settings": () => fail(400, "invalid_thresholds"),
  });
  renderWithSession(<DetectionSettingsPanel />, { api });
  const exportsInput = (await screen.findByLabelText("Threshold: Exports")) as HTMLInputElement;
  expect(exportsInput.value).toBe("99");
  // Reset asks first; declining keeps the edits.
  const confirmSpy = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
  fireEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
  expect(exportsInput.value).toBe("99");
  fireEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
  expect(confirmSpy).toHaveBeenCalledTimes(2);
  confirmSpy.mockRestore();
  expect(exportsInput.value).toBe("20");
  expect(screen.getAllByText(/–150 events · default/).length).toBeGreaterThan(0);
  expect((screen.getByRole("checkbox", { name: "Adaptive thresholds" }) as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Save thresholds" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/rejected the thresholds/));
});

test("validateDetection enforces bounds, integers and lock >= brute force", () => {
  const d = detectionSettings();
  expect(validateDetection(d.effective, d.bounds)).toEqual([]);
  const bad = { ...d.effective, thresholds: { ...d.effective.thresholds, brute_force: 20, spray_users: 2.5 } };
  expect(validateDetection(bad, d.bounds).sort()).toEqual(["brute_force_lock", "spray_users"]);
  expect(validateDetection({ ...d.effective, adaptive_max_factor: 99 }, d.bounds)).toEqual([]);
  expect(validateDetection({ ...d.effective, mode: "adaptive", adaptive_max_factor: 99, baseline_days: 1 }, d.bounds)).toEqual(["adaptive_max_factor", "baseline_days"]);
});
