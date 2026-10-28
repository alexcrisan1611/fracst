#!/usr/bin/env node
// Manage fracst keys on a gateway.
//
//   FRACST_GATEWAY=https://fracst-gateway.<you>.workers.dev FRACST_ADMIN_TOKEN=... \
//     node scripts/keys.mjs create alice [--minutes 600]
//
// Commands: create <name> [--minutes N], list, revoke <name>, sessions,
// selftest, bench.

const gateway = (process.env.FRACST_GATEWAY ?? "").replace(/\/+$/, "");
const token = process.env.FRACST_ADMIN_TOKEN ?? "";
const [command, ...rest] = process.argv.slice(2);

function usage() {
	console.error(`usage: node scripts/keys.mjs <command>

  create <name> [--minutes N]   new key, optionally limited to N GPU minutes a month
  list                          keys and this month's usage
  revoke <name>                 revoke a key (its open sessions end within a minute)
  sessions                      sessions open now
  selftest | bench              run the GPU self-test or benchmark on fal

Needs FRACST_GATEWAY (the gateway's https:// address) and FRACST_ADMIN_TOKEN.`);
	process.exit(2);
}

if (!command || !gateway || !token) usage();

async function call(method, path, body) {
	const response = await fetch(`${gateway}/admin${path}`, {
		method,
		headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
		body: body === undefined ? undefined : JSON.stringify(body),
	});
	const text = await response.text();
	let data;
	try {
		data = JSON.parse(text);
	} catch {
		data = text;
	}
	if (!response.ok) {
		console.error(`error (HTTP ${response.status}):`, data.error ?? data);
		process.exit(1);
	}
	return data;
}

switch (command) {
	case "create": {
		const name = rest[0];
		if (!name) usage();
		const i = rest.indexOf("--minutes");
		const minutes = i >= 0 ? Number(rest[i + 1]) : null;
		const { key } = await call("POST", "/keys", { name, monthly_minutes: minutes });
		console.log(`key for ${name}${minutes === null ? "" : ` (${minutes} GPU minutes a month)`}:\n\n  ${key}\n`);
		console.log("It is shown only once. In the fracst app: Remote GPU → Server:");
		console.log(`  ${gateway.replace(/^http/, "ws")}/stream`);
		console.log("and paste the key under Key.");
		break;
	}
	case "list": {
		const keys = await call("GET", "/keys");
		if (keys.length === 0) console.log("no keys yet");
		for (const k of keys) {
			const limit = k.monthly_minutes === null ? "no limit" : `of ${k.monthly_minutes}`;
			console.log(
				`${k.name.padEnd(20)} ${String(k.used_minutes).padStart(8)} min ${limit.padEnd(10)} ` +
					`${k.sessions_this_month} sessions this month, ${k.open_sessions} open`,
			);
		}
		break;
	}
	case "revoke": {
		if (!rest[0]) usage();
		console.log(await call("DELETE", `/keys/${encodeURIComponent(rest[0])}`));
		break;
	}
	case "sessions":
		console.log(await call("GET", "/sessions"));
		break;
	case "selftest":
	case "bench":
		console.log(JSON.stringify(await call("POST", `/${command}`, {}), null, 2));
		break;
	default:
		usage();
}
