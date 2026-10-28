// fracst-gateway: lets the fracst app render on the fal.ai GPU server with
// a fracst key of its own, so the fal key stays here, as a secret.
//
//   GET  /stream                WebSocket, Authorization: Bearer <fracst key>
//   GET  /health
//   POST /admin/keys            {"name": "...", "monthly_minutes": 600}  -> {"key": "fst_..."}
//   GET  /admin/keys            keys with this month's usage
//   DELETE /admin/keys/<name>   revoke
//   GET  /admin/sessions        sessions open now
//   POST /admin/selftest        run the GPU self-test on fal
//   POST /admin/bench           run the GPU benchmark on fal
//
// Admin requests need Authorization: Bearer <ADMIN_TOKEN>.

import { bearer, hashKey, sameSecret } from "./keys";
import { intVar, upstreamUrl } from "./session";

export { Registry } from "./registry";
export { Session } from "./session";

function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body, null, 2), { status, headers: { "Content-Type": "application/json" } });
}

/** Errors for the app, which shows them as they are. The header is for
 * clients that only see a refused handshake's headers. */
function refuse(message: string, status: number): Response {
	return new Response(message, { status, headers: { "Content-Type": "text/plain", "X-Fracst-Error": message } });
}

function registry(env: Env) {
	return env.REGISTRY.get(env.REGISTRY.idFromName("registry"));
}

async function stream(request: Request, env: Env): Promise<Response> {
	if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
		return refuse("expected a WebSocket connection", 426);
	}
	if (!upstreamUrl(env)) return refuse("the gateway has no GPU server configured", 503);
	const key = bearer(request);
	if (!key) return refuse("send your fracst key as Authorization: Bearer <key>", 401);
	const admission = await registry(env).admit(await hashKey(key), intVar(env.MAX_SESSIONS_PER_KEY, 2));
	if (!admission.ok) return refuse(admission.message, admission.status);
	const headers = new Headers(request.headers);
	headers.delete("Authorization");
	headers.set("X-Fracst-Key", admission.name);
	headers.set("X-Fracst-Session", admission.session);
	const session = env.SESSIONS.get(env.SESSIONS.newUniqueId());
	return session.fetch(new Request(request.url, { headers }));
}

async function admin(request: Request, env: Env, path: string): Promise<Response> {
	if (!env.ADMIN_TOKEN) return json({ error: "set the ADMIN_TOKEN secret first" }, 503);
	const token = bearer(request);
	if (!token || !(await sameSecret(token, env.ADMIN_TOKEN))) return json({ error: "unauthorized" }, 401);
	const reg = registry(env);
	const method = request.method;
	if (path === "/keys" && method === "GET") return json(await reg.listKeys());
	if (path === "/keys" && method === "POST") {
		const body = (await request.json().catch(() => null)) as { name?: unknown; monthly_minutes?: unknown } | null;
		if (!body || typeof body.name !== "string") return json({ error: 'send {"name": "..."}' }, 400);
		const minutes = body.monthly_minutes ?? null;
		if (minutes !== null && typeof minutes !== "number") return json({ error: "monthly_minutes must be a number" }, 400);
		const created = await reg.createKey(body.name, minutes);
		return "error" in created ? json(created, 400) : json({ name: body.name, ...created }, 201);
	}
	const revoke = /^\/keys\/([^/]+)$/.exec(path);
	if (revoke && method === "DELETE") {
		const name = decodeURIComponent(revoke[1]);
		return (await reg.revokeKey(name)) ? json({ revoked: name }) : json({ error: `no key named ${name}` }, 404);
	}
	if (path === "/sessions" && method === "GET") return json(await reg.liveSessions());
	if ((path === "/selftest" || path === "/bench") && method === "POST") {
		if (!env.FAL_APP || !env.FAL_KEY) return json({ error: "needs FAL_APP and the FAL_KEY secret" }, 503);
		const response = await fetch(`https://fal.run/${env.FAL_APP}${path}`, {
			method: "POST",
			headers: { Authorization: `Key ${env.FAL_KEY}`, "Content-Type": "application/json" },
			body: "{}",
		});
		return new Response(response.body, { status: response.status, headers: { "Content-Type": "application/json" } });
	}
	return json({ error: "not found" }, 404);
}

export default {
	async fetch(request, env): Promise<Response> {
		const path = new URL(request.url).pathname;
		if (path === "/stream") return stream(request, env);
		if (path.startsWith("/admin/")) return admin(request, env, path.slice("/admin".length));
		if (path === "/" || path === "/health") {
			return json({ service: "fracst-gateway", ready: upstreamUrl(env) !== null });
		}
		return json({ error: "not found" }, 404);
	},
} satisfies ExportedHandler<Env>;
