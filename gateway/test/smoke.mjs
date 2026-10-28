// Smoke test against a running gateway with a fracst-server behind it:
//   FRACST_GATEWAY=http://127.0.0.1:8787 FRACST_ADMIN_TOKEN=... node test/smoke.mjs
// Checks keys, the per-key session limit, relaying the server's hello, and
// that revoking a key ends its open sessions at their next check-in.

const gateway = process.env.FRACST_GATEWAY.replace(/\/+$/, "");
const admin = { Authorization: `Bearer ${process.env.FRACST_ADMIN_TOKEN}`, "Content-Type": "application/json" };
const wsUrl = gateway.replace(/^http/, "ws") + "/stream";

async function api(method, path, body) {
	const r = await fetch(`${gateway}/admin${path}`, { method, headers: admin, body: body && JSON.stringify(body) });
	return { status: r.status, data: await r.json() };
}

function open(key) {
	return new Promise((resolve) => {
		const ws = new WebSocket(wsUrl, { headers: { Authorization: `Bearer ${key}` } });
		const texts = [];
		let settled = false;
		ws.addEventListener("message", (e) => {
			if (typeof e.data === "string") texts.push(e.data);
			if (!settled && typeof e.data === "string" && JSON.parse(e.data).type === "hello") {
				settled = true;
				resolve({ ok: true, ws, texts });
			}
		});
		ws.addEventListener("close", (e) => {
			ws.closedWith = { code: e.code, reason: e.reason };
			if (!settled) {
				settled = true;
				resolve({ ok: false, ws, texts, close: ws.closedWith });
			}
		});
		ws.addEventListener("error", () => {
			if (!settled) {
				settled = true;
				resolve({ ok: false, ws, texts, error: true });
			}
		});
	});
}

function check(what, ok) {
	console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
	if (!ok) process.exitCode = 1;
}

const name = `smoke-${Date.now()}`;
const created = await api("POST", "/keys", { name });
check("admin creates a key", created.status === 201 && created.data.key.startsWith("fst_"));
const key = created.data.key;
check("duplicate names are refused", (await api("POST", "/keys", { name })).status === 400);
check("admin needs the token", (await fetch(`${gateway}/admin/keys`)).status === 401);

const a = await open(key);
check("a session gets the server's hello", a.ok);
check("status messages come before the hello", a.texts[0] && JSON.parse(a.texts[0]).type === "status");
const b = await open(key);
check("a second session on the same key is allowed", b.ok);
const c = await open(key);
check("a third is refused (MAX_SESSIONS_PER_KEY = 2)", !c.ok);
const sessions = await api("GET", "/sessions");
check("admin sees both open sessions", sessions.data.filter((s) => s.name === name).length === 2);

b.ws.close(1000, "done");
await new Promise((r) => setTimeout(r, 1000));
const d = await open(key);
check("closing one frees a slot", d.ok);
d.ws.close(1000, "done");

check("admin revokes the key", (await api("DELETE", `/keys/${name}`)).status === 200);
const e = await open(key);
check("a revoked key cannot connect", !e.ok);
console.log("     waiting up to 75 s for the open session's next check-in...");
const deadline = Date.now() + 75_000;
while (!a.ws.closedWith && Date.now() < deadline) await new Promise((r) => setTimeout(r, 500));
check(`the open session of a revoked key is closed (${a.ws.closedWith?.reason ?? "still open"})`, !!a.ws.closedWith);
