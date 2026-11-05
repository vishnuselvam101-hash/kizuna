import type { Perm } from "./session";

export type View =
  | "dashboard"
  | "documents"
  | "records"
  | "revisions"
  | "findings"
  | "decisions"
  | "affected"
  | "reports"
  | "users"
  | "integrations"
  | "security"
  | "account";

export interface Section {
  id: "overview" | "evidence" | "conflicts" | "decisions" | "reports" | "admin";
  views: { id: View; perm?: Perm }[];
}

export const SECTIONS: Section[] = [
  { id: "overview", views: [{ id: "dashboard" }] },
  { id: "evidence", views: [{ id: "documents" }, { id: "records" }, { id: "revisions" }] },
  { id: "conflicts", views: [{ id: "findings" }] },
  { id: "decisions", views: [{ id: "decisions" }, { id: "affected" }] },
  { id: "reports", views: [{ id: "reports" }] },
  { id: "admin", views: [{ id: "users", perm: "user_read" }, { id: "integrations", perm: "user_manage" }, { id: "security", perm: "audit_read" }] },
];

/** The section a view belongs to; `account` (own security settings) sits outside the section bar. */
export const sectionOf = (v: View): Section | undefined => SECTIONS.find((s) => s.views.some((x) => x.id === v));
