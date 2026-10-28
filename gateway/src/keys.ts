// fracst keys: "fst_" and 32 random bytes in base64url. Only their SHA-256
// hashes are stored.

export function newKey(): string {
	const bytes = crypto.getRandomValues(new Uint8Array(32));
	let binary = "";
	for (const b of bytes) binary += String.fromCharCode(b);
	return "fst_" + btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sha256(text: string): Promise<ArrayBuffer> {
	return crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
}

export async function hashKey(key: string): Promise<string> {
	return [...new Uint8Array(await sha256(key))].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** The token of an `Authorization: Bearer <token>` header. */
export function bearer(request: Request): string | null {
	const match = /^Bearer\s+(\S+)$/i.exec((request.headers.get("Authorization") ?? "").trim());
	return match ? match[1] : null;
}

/** Compare secrets without leaking where they differ through timing. */
export async function sameSecret(a: string, b: string): Promise<boolean> {
	const [x, y] = await Promise.all([sha256(a), sha256(b)]);
	return crypto.subtle.timingSafeEqual(x, y);
}

export const KEY_NAME = /^[A-Za-z0-9_.-]{1,64}$/;
