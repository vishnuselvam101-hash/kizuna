import { useState } from "react";
import { ApiError, type Api } from "./api";

export function LoginView({ api, onLogin }: { api: Api; onLogin: () => void }) {
  const [f, setF] = useState({ tenant: "", email: "", password: "", totp: "" });
  const [needTotp, setNeedTotp] = useState(false);
  const [err, setErr] = useState("");
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    try {
      await api.login(f.tenant, f.email, f.password, f.totp || undefined);
      setF({ ...f, password: "", totp: "" });
      onLogin();
    } catch (x) {
      if (x instanceof ApiError && x.code === "mfa_required") setNeedTotp(true);
      else setErr(x instanceof ApiError && x.status === 429 ? "Too many attempts, wait a minute." : "Sign-in failed.");
    }
  }
  const field = (k: keyof typeof f, label: string, type = "text") => (
    <label>
      {label}
      <input type={type} value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} autoComplete="off" />
    </label>
  );
  return (
    <form onSubmit={submit} className="login">
      <h1>KIZUNA Verify</h1>
      {field("tenant", "Organisation")}
      {field("email", "Email", "email")}
      {field("password", "Password", "password")}
      {needTotp && field("totp", "Authenticator code")}
      <button>Sign in</button>
      {err && <p role="alert">{err}</p>}
    </form>
  );
}
