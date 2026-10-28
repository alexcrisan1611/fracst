// Secrets, set with `wrangler secret put` (or .dev.vars locally).
interface Env {
	/** The fal.ai key the gateway connects with. */
	FAL_KEY?: string;
	/** Bearer token for the /admin endpoints. */
	ADMIN_TOKEN?: string;
}
