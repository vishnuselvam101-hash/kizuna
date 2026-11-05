import { fireEvent, render, screen } from "@testing-library/react";
import { I18nProvider } from "./i18n";
import { ActionRow } from "./SecurityView";

test("high-impact actions need explicit approval; executed ones can be reverted", () => {
  const calls: string[] = [];
  const row = (a: Parameters<typeof ActionRow>[0]["a"]) => (
    <I18nProvider>
      <table>
        <tbody>
          <ActionRow a={a} canManage onDecide={(_, v) => calls.push(v)} />
        </tbody>
      </table>
    </I18nProvider>
  );
  const { rerender } = render(row({ id: "a1", kind: "lock_account", target: "u1", policy: "requires_approval", status: "proposed", detail: null }));
  expect(screen.getByText("needs approval")).toBeTruthy();
  fireEvent.click(screen.getByText("Approve"));
  rerender(row({ id: "a2", kind: "block_ip", target: "203.0.113.9", policy: "pre_authorised", status: "executed", detail: "blocked" }));
  fireEvent.click(screen.getByText("Revert"));
  rerender(row({ id: "a3", kind: "revoke_user_sessions", target: "u2", policy: "pre_authorised", status: "executed", detail: "2 sessions revoked" }));
  expect(screen.queryByText("Revert")).toBeNull();
  expect(calls).toEqual(["approve", "revert"]);
});
