import { getAssertion } from "./webauthn";

// API client. Tokens live only in memory (never localStorage) to limit XSS token theft.

export class ApiError extends Error {
  constructor(public status: number, public code: string) {
    super(`${status} ${code}`);
  }
}

export interface Tokens {
  access_token: string;
  refresh_token: string;
}

export class Api {
  private tokens: Tokens | null = null;
  /** Called when the session ends without an explicit logout (refresh failed). */
  onSessionEnd: (() => void) | null = null;
  constructor(private base = "/api/v1", private fetchFn: typeof fetch = (...a) => fetch(...a)) {}

  get authenticated(): boolean {
    return this.tokens !== null;
  }

  setTokens(t: Tokens | null) {
    this.tokens = t;
  }

  async request<T>(method: string, path: string, body?: unknown, retry = true): Promise<T> {
    const headers: Record<string, string> = {};
    if (body !== undefined && !(body instanceof FormData)) headers["content-type"] = "application/json";
    if (this.tokens) headers.authorization = `Bearer ${this.tokens.access_token}`;
    const res = await this.fetchFn(this.base + path, {
      method,
      headers,
      body: body === undefined ? undefined : body instanceof FormData ? body : JSON.stringify(body),
    });
    if (res.status === 401 && retry && this.tokens && path !== "/auth/refresh") {
      if (await this.refresh()) return this.request<T>(method, path, body, false);
    }
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(res.status, (data as { error?: string }).error ?? "unknown");
    return data as T;
  }

  /** Binary download (e.g. an evidence pack ZIP); JSON errors become ApiError like `request`. */
  async blob(method: string, path: string, body?: unknown, retry = true): Promise<Blob> {
    const headers: Record<string, string> = {};
    if (body !== undefined) headers["content-type"] = "application/json";
    if (this.tokens) headers.authorization = `Bearer ${this.tokens.access_token}`;
    const res = await this.fetchFn(this.base + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    if (res.status === 401 && retry && this.tokens) {
      if (await this.refresh()) return this.blob(method, path, body, false);
    }
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      throw new ApiError(res.status, (data as { error?: string }).error ?? "unknown");
    }
    return res.blob();
  }

  async text(path: string): Promise<string> {
    const headers: Record<string, string> = {};
    if (this.tokens) headers.authorization = `Bearer ${this.tokens.access_token}`;
    const res = await this.fetchFn(this.base + path, { method: "GET", headers });
    if (!res.ok) throw new ApiError(res.status, "request_failed");
    return res.text();
  }

  private async refresh(): Promise<boolean> {
    const rt = this.tokens?.refresh_token;
    this.tokens = null;
    if (rt) {
      try {
        this.tokens = await this.request<Tokens>("POST", "/auth/refresh", { refresh_token: rt }, false);
        return true;
      } catch {
        /* fall through: session is over */
      }
    }
    this.onSessionEnd?.();
    return false;
  }

  async login(tenant: string, email: string, password: string, totp_code?: string) {
    const t = await this.request<Tokens & { mfa_verified: boolean }>("POST", "/auth/login", {
      tenant,
      email,
      password,
      totp_code: totp_code || null,
    });
    this.tokens = t;
    return t;
  }

  /** Password first, then the security key (navigator.credentials.get); yields an MFA-verified session. */
  async loginWithKey(tenant: string, email: string, password: string) {
    const start = await this.request<{ challenge_id: string; public_key: { publicKey: Record<string, unknown> } }>(
      "POST",
      "/auth/webauthn/login/start",
      { tenant, email, password },
    );
    const credential = await getAssertion(start.public_key);
    const t = await this.request<Tokens & { mfa_verified: boolean }>("POST", "/auth/webauthn/login/finish", {
      tenant,
      challenge_id: start.challenge_id,
      credential,
    });
    this.tokens = t;
    return t;
  }

  /** Lost security key: password + one-time recovery code. Clears the keys; the session is not MFA-verified. */
  async recover(tenant: string, email: string, password: string, recovery_code: string) {
    const t = await this.request<Tokens & { mfa_verified: boolean }>("POST", "/auth/webauthn/recover", { tenant, email, password, recovery_code });
    this.tokens = t;
    return t;
  }

  async logout() {
    try {
      await this.request("POST", "/auth/logout", {});
    } finally {
      this.tokens = null;
    }
  }
}
