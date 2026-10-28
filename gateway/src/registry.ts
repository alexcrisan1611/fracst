// The registry: one Durable Object holding the keys (as hashes), how many
// minutes each has used per month, and the sessions open right now.

import { DurableObject } from "cloudflare:workers";
import { KEY_NAME, hashKey, newKey } from "./keys";

export type Admission = { ok: true; name: string; session: string } | { ok: false; status: number; message: string };

export interface KeyInfo {
	name: string;
	created: string;
	/** Monthly limit, or null for none. */
	monthly_minutes: number | null;
	used_minutes: number;
	sessions_this_month: number;
	open_sessions: number;
}

export interface LiveSession {
	session: string;
	name: string;
	started: string;
	minutes: number;
}

/** Sessions that have not checked in for this long are presumed dead. */
const STALE_MS = 3 * 60_000;

function month(now = Date.now()): string {
	return new Date(now).toISOString().slice(0, 7);
}

export class Registry extends DurableObject<Env> {
	private sql: SqlStorage;

	constructor(ctx: DurableObjectState, env: Env) {
		super(ctx, env);
		this.sql = ctx.storage.sql;
		this.sql.exec(`CREATE TABLE IF NOT EXISTS keys (
			name TEXT PRIMARY KEY,
			hash TEXT NOT NULL UNIQUE,
			created INTEGER NOT NULL,
			monthly_minutes INTEGER
		)`);
		this.sql.exec(`CREATE TABLE IF NOT EXISTS usage (
			name TEXT NOT NULL,
			month TEXT NOT NULL,
			seconds REAL NOT NULL DEFAULT 0,
			sessions INTEGER NOT NULL DEFAULT 0,
			PRIMARY KEY (name, month)
		)`);
		this.sql.exec(`CREATE TABLE IF NOT EXISTS live (
			session TEXT PRIMARY KEY,
			name TEXT NOT NULL,
			started INTEGER NOT NULL,
			seen INTEGER NOT NULL
		)`);
	}

	private usedSeconds(name: string): number {
		const row = this.sql
			.exec<{ seconds: number }>("SELECT seconds FROM usage WHERE name = ? AND month = ?", name, month())
			.toArray()[0];
		return row?.seconds ?? 0;
	}

	private addUsage(name: string, seconds: number, newSession: boolean): void {
		this.sql.exec(
			`INSERT INTO usage (name, month, seconds, sessions) VALUES (?, ?, ?, ?)
			 ON CONFLICT (name, month) DO UPDATE SET seconds = seconds + excluded.seconds, sessions = sessions + excluded.sessions`,
			name,
			month(),
			Math.max(0, seconds),
			newSession ? 1 : 0,
		);
	}

	private dropStale(): void {
		this.sql.exec("DELETE FROM live WHERE seen < ?", Date.now() - STALE_MS);
	}

	/** Why `name` may not (or no longer) render, or null if it may. */
	private refusal(name: string, monthlyMinutes: number | null): string | null {
		if (monthlyMinutes !== null && this.usedSeconds(name) >= monthlyMinutes * 60) {
			return `this key has used its ${monthlyMinutes} GPU minutes for ${month()}`;
		}
		return null;
	}

	// ---- keys (admin) ----

	async createKey(name: string, monthlyMinutes: number | null): Promise<{ key: string } | { error: string }> {
		if (!KEY_NAME.test(name)) return { error: "names are 1-64 letters, digits, '.', '_' or '-'" };
		if (monthlyMinutes !== null && !(Number.isInteger(monthlyMinutes) && monthlyMinutes >= 0)) {
			return { error: "monthly_minutes must be a whole number of minutes, or null" };
		}
		if (this.sql.exec("SELECT 1 FROM keys WHERE name = ?", name).toArray().length > 0) {
			return { error: `a key named ${name} exists; revoke it first` };
		}
		const key = newKey();
		this.sql.exec(
			"INSERT INTO keys (name, hash, created, monthly_minutes) VALUES (?, ?, ?, ?)",
			name,
			await hashKey(key),
			Date.now(),
			monthlyMinutes,
		);
		return { key };
	}

	listKeys(): KeyInfo[] {
		this.dropStale();
		return this.sql
			.exec<{ name: string; created: number; monthly_minutes: number | null }>(
				"SELECT name, created, monthly_minutes FROM keys ORDER BY name",
			)
			.toArray()
			.map((k) => {
				const usage = this.sql
					.exec<{ seconds: number; sessions: number }>(
						"SELECT seconds, sessions FROM usage WHERE name = ? AND month = ?",
						k.name,
						month(),
					)
					.toArray()[0];
				const open = this.sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM live WHERE name = ?", k.name).one().n;
				return {
					name: k.name,
					created: new Date(k.created).toISOString(),
					monthly_minutes: k.monthly_minutes,
					used_minutes: Math.round(((usage?.seconds ?? 0) / 60) * 10) / 10,
					sessions_this_month: usage?.sessions ?? 0,
					open_sessions: open,
				};
			});
	}

	/** Revoke a key. Its open sessions end at their next check-in. */
	revokeKey(name: string): boolean {
		const removed = this.sql.exec("DELETE FROM keys WHERE name = ?", name).rowsWritten > 0;
		return removed;
	}

	liveSessions(): LiveSession[] {
		this.dropStale();
		const now = Date.now();
		return this.sql
			.exec<{ session: string; name: string; started: number }>(
				"SELECT session, name, started FROM live ORDER BY started",
			)
			.toArray()
			.map((s) => ({
				session: s.session,
				name: s.name,
				started: new Date(s.started).toISOString(),
				minutes: Math.round(((now - s.started) / 60_000) * 10) / 10,
			}));
	}

	// ---- sessions ----

	/** Check a key (by hash) and, if it may render, open a session for it. */
	admit(hash: string, maxSessions: number): Admission {
		const key = this.sql
			.exec<{ name: string; monthly_minutes: number | null }>(
				"SELECT name, monthly_minutes FROM keys WHERE hash = ?",
				hash,
			)
			.toArray()[0];
		if (!key) return { ok: false, status: 401, message: "unknown or revoked fracst key" };
		const refusal = this.refusal(key.name, key.monthly_minutes);
		if (refusal) return { ok: false, status: 429, message: refusal };
		this.dropStale();
		const open = this.sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM live WHERE name = ?", key.name).one().n;
		if (open >= maxSessions) {
			return { ok: false, status: 429, message: `this key already has ${open} open session(s), the most allowed` };
		}
		const session = crypto.randomUUID();
		const now = Date.now();
		this.sql.exec("INSERT INTO live (session, name, started, seen) VALUES (?, ?, ?, ?)", session, key.name, now, now);
		this.addUsage(key.name, 0, true);
		return { ok: true, name: key.name, session };
	}

	/** A session checks in, adding `seconds` of use. Returns a reason to stop, if any. */
	heartbeat(session: string, name: string, seconds: number): { stop: string | null } {
		this.addUsage(name, seconds, false);
		this.sql.exec("UPDATE live SET seen = ? WHERE session = ?", Date.now(), session);
		const key = this.sql
			.exec<{ monthly_minutes: number | null }>("SELECT monthly_minutes FROM keys WHERE name = ?", name)
			.toArray()[0];
		if (!key) return { stop: "this fracst key was revoked" };
		return { stop: this.refusal(name, key.monthly_minutes) };
	}

	end(session: string, name: string, seconds: number): void {
		this.addUsage(name, seconds, false);
		this.sql.exec("DELETE FROM live WHERE session = ?", session);
	}
}
