import type { Perm } from "./session";

export type View =
  | "dashboard"
  | "documents"
  | "records"
  | "revisions"
  | "findings"
  | "match"
  | "decisions"
  | "affected"
  | "rerequests"
  | "reports"
  | "users"
  | "integrations"
  | "security";

export interface Section {
  id: "overview" | "evidence" | "conflicts" | "decisions" | "reports" | "admin";
  views: { id: View; perm?: Perm }[];
}

export const SECTIONS: Section[] = [
  { id: "overview", views: [{ id: "dashboard" }] },
  { id: "evidence", views: [{ id: "documents" }, { id: "records" }, { id: "revisions" }] },
  { id: "conflicts", views: [{ id: "findings" }, { id: "match" }] },
  { id: "decisions", views: [{ id: "decisions" }, { id: "affected" }, { id: "rerequests" }] },
  { id: "reports", views: [{ id: "reports" }] },
  { id: "admin", views: [{ id: "users", perm: "user_read" }, { id: "integrations", perm: "user_manage" }, { id: "security", perm: "audit_read" }] },
];

export const sectionOf = (v: View) => SECTIONS.find((s) => s.views.some((x) => x.id === v))!;
