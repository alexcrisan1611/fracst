// A session: one Durable Object per app connection. It opens the WebSocket
// to the GPU server (on Modal or fal, with the gateway's token) and relays
// messages both ways unchanged, so the app speaks the fracst protocol end
// to end. While the GPU starts it sends the app status messages, and it
// checks in with the registry every minute to count usage.

import { DurableObject } from "cloudflare:workers";

/** How long to keep trying to reach the GPU server (a cold start can take minutes). */
const CONNECT_TIMEOUT_MS = 5 * 60_000;
const STATUS_EVERY_MS = 5_000;
const HEARTBEAT_MS = 60_000;
/** Requests from the app are small JSON texts. */
const MAX_REQUEST_BYTES = 64 * 1024;

class Refused extends Error {}

/** The fracst-server WebSocket address, as an http(s) URL for fetch(). */
export function upstreamUrl(env: Env): string | null {
	const url = env.UPSTREAM_URL || (env.FAL_APP ? `https://fal.run/${env.FAL_APP}/stream` : "");
	return url ? url.replace(/^ws(s?):\/\//, "http$1://") : null;
}

export function intVar(value: string | undefined, fallback: number): number {
	const n = Number.parseInt(value ?? "", 10);
	return Number.isFinite(n) && n > 0 ? n : fallback;
}

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Close reasons are limited to 123 bytes. */
function reasonText(reason: string): string {
	const bytes = new TextEncoder().encode(reason);
	return bytes.length <= 123 ? reason : new TextDecoder().decode(bytes.slice(0, 120)).replace(/�$/, "") + "…";
}

export class Session extends DurableObject<Env> {
	async fetch(request: Request): Promise<Response> {
		const name = request.headers.get("X-Fracst-Key");
		const session = request.headers.get("X-Fracst-Session");
		if (!name || !session) return new Response("sessions are opened by the gateway", { status: 400 });
		const pair = new WebSocketPair();
		const [client, socket] = [pair[0], pair[1]];
		socket.accept();
		void this.relay(socket, name, session);
		return new Response(null, { status: 101, webSocket: client });
	}

	private async relay(client: WebSocket, name: string, session: string): Promise<void> {
		const registry = this.env.REGISTRY.get(this.env.REGISTRY.idFromName("registry"));
		const started = Date.now();
		let counted = started;
		let upstream: WebSocket | null = null;
		let closed = false;
		const queued: string[] = [];
		let heartbeat: number | null = null;
		let ticker: number | null = null;
		const tell = (message: object) => {
			try {
				client.send(JSON.stringify(message));
			} catch {
				// The app is gone; the close handler cleans up.
			}
		};

		const finish = async (reason: string, error = false) => {
			if (closed) return;
			closed = true;
			clearInterval(heartbeat);
			clearInterval(ticker);
			if (error) tell({ type: "error", id: null, message: reason });
			try {
				client.close(error ? 4000 : 1000, reasonText(reason));
			} catch {}
			try {
				upstream?.close(1000, "the app disconnected");
			} catch {}
			const seconds = (Date.now() - counted) / 1000;
			counted = Date.now();
			await registry.end(session, name, seconds);
		};

		client.addEventListener("message", (event) => {
			if (typeof event.data !== "string" || event.data.length > MAX_REQUEST_BYTES) {
				void finish("the gateway only relays small text requests", true);
			} else if (upstream) {
				upstream.send(event.data);
			} else {
				queued.push(event.data);
			}
		});
		client.addEventListener("close", () => void finish("the app disconnected"));
		client.addEventListener("error", () => void finish("the app's connection failed"));

		const maxMs = intVar(this.env.MAX_SESSION_MINUTES, 120) * 60_000;
		heartbeat = setInterval(async () => {
			const now = Date.now();
			const { stop } = await registry.heartbeat(session, name, (now - counted) / 1000);
			counted = now;
			if (stop) {
				await finish(stop, true);
			} else if (now - started >= maxMs) {
				await finish(`sessions last at most ${maxMs / 60_000} minutes; reconnect to continue`, true);
			}
		}, HEARTBEAT_MS);

		tell({ type: "status", message: "Connecting to the GPU server…" });
		ticker = setInterval(() => {
			const s = Math.round((Date.now() - started) / 1000);
			tell({ type: "status", message: `Waiting for a GPU (an idle one takes a minute or two to start)… ${s} s` });
		}, STATUS_EVERY_MS);

		try {
			upstream = await this.openUpstream(() => closed);
		} catch (e) {
			await finish(e instanceof Error ? e.message : String(e), true);
			return;
		} finally {
			clearInterval(ticker);
		}
		if (closed) {
			upstream.close(1000, "the app disconnected");
			return;
		}
		upstream.addEventListener("message", (event) => {
			if (!closed) client.send(event.data as string | ArrayBuffer);
		});
		upstream.addEventListener("close", (event) => {
			void finish(event.reason || "the GPU server closed the connection", event.code !== 1000);
		});
		upstream.addEventListener("error", () => void finish("the connection to the GPU server failed", true));
		for (const message of queued) upstream.send(message);
		queued.length = 0;
	}

	/** Open the WebSocket to the GPU server, retrying while it starts. */
	private async openUpstream(cancelled: () => boolean): Promise<WebSocket> {
		const url = upstreamUrl(this.env);
		if (!url) throw new Refused("the gateway has no GPU server configured (UPSTREAM_URL or FAL_APP)");
		const headers: Record<string, string> = { Upgrade: "websocket" };
		if (this.env.MODAL_KEY && this.env.MODAL_SECRET) {
			headers["Modal-Key"] = this.env.MODAL_KEY;
			headers["Modal-Secret"] = this.env.MODAL_SECRET;
		} else if (this.env.FAL_KEY) {
			headers.Authorization = `Key ${this.env.FAL_KEY}`;
		}
		const deadline = Date.now() + CONNECT_TIMEOUT_MS;
		let last = "";
		while (Date.now() < deadline && !cancelled()) {
			try {
				const response = await fetch(url, { headers });
				const ws = response.webSocket;
				if (ws) {
					ws.binaryType = "arraybuffer";
					ws.accept();
					return ws;
				}
				const body = (await response.text()).slice(0, 200).trim();
				if (response.status === 401 || response.status === 403) {
					throw new Refused(`the GPU server refused the gateway's token (HTTP ${response.status})`);
				}
				if (response.status === 404) throw new Refused(`no GPU server at ${url} (HTTP 404): check UPSTREAM_URL or FAL_APP`);
				last = `HTTP ${response.status}${body ? `: ${body}` : ""}`;
			} catch (e) {
				if (e instanceof Refused) throw e;
				last = e instanceof Error ? e.message : String(e);
			}
			await sleep(3000);
		}
		throw new Error(`could not reach the GPU server (${last || "timed out"})`);
	}
}
