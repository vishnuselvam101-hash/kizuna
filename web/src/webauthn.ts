// WebAuthn helpers: the server (webauthn-rs) sends options with base64url strings; the browser API wants bytes.

const b64uToBytes = (s: string): ArrayBuffer => {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  const out = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
  return out.buffer;
};
const bytesToB64u = (buf: ArrayBuffer | null): string | null => {
  if (!buf) return null;
  let s = "";
  new Uint8Array(buf).forEach((x) => (s += String.fromCharCode(x)));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

type Json = Record<string, unknown>;
type CredDesc = { id: string } & Json;

export const webauthnSupported = () => typeof window !== "undefined" && !!window.PublicKeyCredential && !!navigator.credentials;

/** Runs navigator.credentials.create for a webauthn-rs CreationChallengeResponse; returns the JSON to post back. */
export async function createCredential(ccr: { publicKey: Json }): Promise<Json> {
  const pk = ccr.publicKey as Json & { challenge: string; user: Json & { id: string }; excludeCredentials?: CredDesc[] };
  const options: PublicKeyCredentialCreationOptions = {
    ...(pk as unknown as PublicKeyCredentialCreationOptions),
    challenge: b64uToBytes(pk.challenge),
    user: { ...(pk.user as unknown as PublicKeyCredentialUserEntity), id: b64uToBytes(pk.user.id) },
    excludeCredentials: (pk.excludeCredentials ?? []).map((c) => ({ ...(c as unknown as PublicKeyCredentialDescriptor), id: b64uToBytes(c.id) })),
  };
  const cred = (await navigator.credentials.create({ publicKey: options })) as PublicKeyCredential | null;
  if (!cred) throw new Error("no_credential");
  const r = cred.response as AuthenticatorAttestationResponse;
  return {
    id: cred.id,
    rawId: bytesToB64u(cred.rawId),
    type: cred.type,
    response: { attestationObject: bytesToB64u(r.attestationObject), clientDataJSON: bytesToB64u(r.clientDataJSON) },
    extensions: cred.getClientExtensionResults?.() ?? {},
  };
}

/** Runs navigator.credentials.get for a webauthn-rs RequestChallengeResponse; returns the JSON to post back. */
export async function getAssertion(rcr: { publicKey: Json }, signal?: AbortSignal): Promise<Json> {
  const pk = rcr.publicKey as Json & { challenge: string; allowCredentials?: CredDesc[] };
  const options: PublicKeyCredentialRequestOptions = {
    ...(pk as unknown as PublicKeyCredentialRequestOptions),
    challenge: b64uToBytes(pk.challenge),
    allowCredentials: (pk.allowCredentials ?? []).map((c) => ({ ...(c as unknown as PublicKeyCredentialDescriptor), id: b64uToBytes(c.id) })),
  };
  const cred = (await navigator.credentials.get({ publicKey: options, signal })) as PublicKeyCredential | null;
  if (!cred) throw new Error("no_credential");
  const r = cred.response as AuthenticatorAssertionResponse;
  return {
    id: cred.id,
    rawId: bytesToB64u(cred.rawId),
    type: cred.type,
    response: {
      authenticatorData: bytesToB64u(r.authenticatorData),
      clientDataJSON: bytesToB64u(r.clientDataJSON),
      signature: bytesToB64u(r.signature),
      userHandle: bytesToB64u(r.userHandle),
    },
    extensions: cred.getClientExtensionResults?.() ?? {},
  };
}
