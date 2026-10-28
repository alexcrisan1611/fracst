// Secrets, set with `wrangler secret put` (or .dev.vars locally).
interface Env {
	/** The fal.ai key the gateway connects with. */
	FAL_KEY?: string;
	/** A Modal proxy auth token (wk-…, ws-…), for a GPU server on Modal. */
	MODAL_KEY?: string;
	MODAL_SECRET?: string;
	/** Bearer token for the /admin endpoints. */
	ADMIN_TOKEN?: string;
}
