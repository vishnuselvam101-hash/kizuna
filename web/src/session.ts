import { createContext, useContext } from "react";
import type { Api } from "./api";
import type { Me } from "./types";

// Mirrors policy-engine role_grants for UI affordances only; the server remains the authority.
export type Perm =
  | "user_read"
  | "user_manage"
  | "audit_read"
  | "audit_verify"
  | "artifact_upload"
  | "claim_write"
  | "review_decide"
  | "decision_write"
  | "incident_manage";

const GRANTS: Record<string, Perm[] | "all"> = {
  tenant_admin: "all",
  auditor: ["user_read", "audit_read", "audit_verify"],
  reviewer: ["artifact_upload", "claim_write", "review_decide", "decision_write"],
  analyst: ["artifact_upload", "claim_write"],
  viewer: [],
};

export const ROLES = ["tenant_admin", "reviewer", "analyst", "auditor", "viewer"] as const;

export function roleCan(role: string, p: Perm): boolean {
  const g = GRANTS[role];
  return g === "all" || (g ?? []).includes(p);
}

export interface Session {
  api: Api;
  me: Me;
  can: (p: Perm) => boolean;
}

export const SessionContext = createContext<Session | null>(null);

export function useSession(): Session {
  const s = useContext(SessionContext);
  if (!s) throw new Error("SessionContext missing");
  return s;
}
