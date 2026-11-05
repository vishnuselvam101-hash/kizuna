// WebAuthn glue: the server (webauthn-rs) sends options with base64url binary fields and expects the credential back
// as JSON with base64url fields. Nothing here is stored; ceremonies run only on explicit user action.

type Json = Record<string, unknown>;

export function b64urlToBuf(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "=");
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

export function bufToB64url(b: ArrayBuffer | ArrayBufferView | null | undefined): string | null {
  if (!b) return null;
  const u8 = b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
  let bin = "";
  for (const x of u8) bin += String.fromCharCode(x);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const withIds = (list: unknown) => (Array.isArray(list) ? list.map((c: Json) => ({ ...c, id: b64urlToBuf(c.id as string) })) : undefined);

/** `{ publicKey: … }` from /register/start → options for navigator.credentials.create. */
export function creationOptions(server: { publicKey: Json }): CredentialCreationOptions {
  const pk = server.publicKey;
  const user = pk.user as Json;
  return {
    publicKey: {
      ...(pk as unknown as PublicKeyCredentialCreationOptions),
      challenge: b64urlToBuf(pk.challenge as string),
      user: { ...(user as unknown as PublicKeyCredentialUserEntity), id: b64urlToBuf(user.id as string) },
      excludeCredentials: withIds(pk.excludeCredentials) as PublicKeyCredentialDescriptor[] | undefined,
    },
  };
}

/** `{ publicKey: … }` from /login/start → options for navigator.credentials.get. */
export function requestOptions(server: { publicKey: Json }): CredentialRequestOptions {
  const pk = server.publicKey;
  return {
    publicKey: {
      ...(pk as unknown as PublicKeyCredentialRequestOptions),
      challenge: b64urlToBuf(pk.challenge as string),
      allowCredentials: withIds(pk.allowCredentials) as PublicKeyCredentialDescriptor[] | undefined,
    },
  };
}

interface Cred {
  id: string;
  rawId: ArrayBuffer;
  type: string;
  response: Record<string, unknown>;
  getClientExtensionResults?: () => unknown;
}

export function registrationJson(c: Cred) {
  const r = c.response as { attestationObject: ArrayBuffer; clientDataJSON: ArrayBuffer; getTransports?: () => string[] };
  return {
    id: c.id,
    rawId: bufToB64url(c.rawId),
    type: c.type,
    extensions: c.getClientExtensionResults?.() ?? {},
    response: {
      attestationObject: bufToB64url(r.attestationObject),
      clientDataJSON: bufToB64url(r.clientDataJSON),
      transports: r.getTransports?.() ?? undefined,
    },
  };
}

export function assertionJson(c: Cred) {
  const r = c.response as { authenticatorData: ArrayBuffer; clientDataJSON: ArrayBuffer; signature: ArrayBuffer; userHandle: ArrayBuffer | null };
  return {
    id: c.id,
    rawId: bufToB64url(c.rawId),
    type: c.type,
    extensions: c.getClientExtensionResults?.() ?? {},
    response: {
      authenticatorData: bufToB64url(r.authenticatorData),
      clientDataJSON: bufToB64url(r.clientDataJSON),
      signature: bufToB64url(r.signature),
      userHandle: bufToB64url(r.userHandle),
    },
  };
}

export const webauthnAvailable = () => typeof window !== "undefined" && !!window.PublicKeyCredential && !!navigator.credentials;

/** Throws "webauthn_cancelled" when the user dismisses the browser prompt or no key answers. */
export async function getAssertion(server: { publicKey: Json }) {
  if (!webauthnAvailable()) throw new Error("webauthn_unavailable");
  const c = (await navigator.credentials.get(requestOptions(server)).catch(() => null)) as Cred | null;
  if (!c) throw new Error("webauthn_cancelled");
  return assertionJson(c);
}

export async function createCredential(server: { publicKey: Json }) {
  if (!webauthnAvailable()) throw new Error("webauthn_unavailable");
  const c = (await navigator.credentials.create(creationOptions(server)).catch(() => null)) as Cred | null;
  if (!c) throw new Error("webauthn_cancelled");
  return registrationJson(c);
}
