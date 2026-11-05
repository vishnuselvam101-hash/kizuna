import { fireEvent, render, screen } from "@testing-library/react";
import { ActionRow } from "./SecurityView";

test("high-impact actions need explicit approval; executed ones can be reverted", () => {
  const calls: string[] = [];
  const { rerender } = render(
    <table><tbody><ActionRow a={{ id: "a1", kind: "lock_account", target: "u1", policy: "requires_approval", status: "proposed", detail: null }} onDecide={(_, v) => calls.push(v)} /></tbody></table>,
  );
  expect(screen.getByText("needs approval")).toBeTruthy();
  fireEvent.click(screen.getByText("Approve"));
  rerender(
    <table><tbody><ActionRow a={{ id: "a2", kind: "block_ip", target: "203.0.113.9", policy: "pre_authorised", status: "executed", detail: "blocked" }} onDecide={(_, v) => calls.push(v)} /></tbody></table>,
  );
  fireEvent.click(screen.getByText("Revert"));
  expect(calls).toEqual(["approve", "revert"]);
});
