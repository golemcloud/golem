import { createHmac, timingSafeEqual } from 'node:crypto'
import { execFile } from 'node:child_process'
import { mkdir, readFile, writeFile, rename } from 'node:fs/promises'
import { isAbsolute, join } from 'node:path'
import { promisify } from 'node:util'
import type { PluginAPI, WebhookEvent, WebhookHandlerContext } from '@ampcode/plugin'

export const description = 'Queue signed Main validation completions in the existing observer without periodic LLM polling.'

const repository = 'golemcloud/golem'
const workflow = '.github/workflows/main-validation.yaml'

interface Config {
	ownerThread: string
	stateDir: string
	secret: string
}

async function readJSON(path: string): Promise<any | undefined> {
	try { return JSON.parse(await readFile(path, 'utf8')) }
	catch (error: any) { if (error.code === 'ENOENT') return undefined; throw error }
}

async function store(path: string, value: unknown) {
	await writeFile(path + '.tmp', JSON.stringify(value), { mode: 0o600 })
	await rename(path + '.tmp', path)
}

/** Only numeric identities cross from the delivery into the agent's instructions. */
export function completion(event: WebhookEvent, secret: string) {
	const signature = event.headers['x-hub-signature-256'] || ''
	if (!/^sha256=[0-9a-f]{64}$/.test(signature)) return
	const expected = createHmac('sha256', secret).update(event.body).digest()
	if (!timingSafeEqual(expected, Buffer.from(signature.slice(7), 'hex'))) return
	if (event.headers['x-github-event'] !== 'workflow_run') return
	let payload: any
	try { payload = JSON.parse(Buffer.from(event.body).toString('utf8')) } catch { return }
	const run = payload?.workflow_run
	if (payload?.repository?.full_name !== repository || payload.action !== 'completed'
		|| run?.repository?.full_name !== repository || run?.path !== workflow
		|| run?.status !== 'completed' || !['schedule', 'workflow_dispatch'].includes(run.event)
		|| !Number.isSafeInteger(run.id) || run.id < 1
		|| !Number.isSafeInteger(run.run_attempt) || run.run_attempt < 1) return
	return { run: run.id, attempt: run.run_attempt }
}

export function receiver(config: Config, api: (path: string, signal: AbortSignal) => Promise<any>) {
	// One plugin instance serializes deliveries; only its owning thread writes this inbox.
	let tail = Promise.resolve()
	return (event: WebhookEvent, ctx: WebhookHandlerContext) => {
		const work = tail.then(async () => {
			ctx.signal.throwIfAborted()
			const id = completion(event, config.secret)
			if (!id) return
			if (ctx.thread.id !== config.ownerThread) throw new Error('Observer webhook owner mismatch')
			const key = `${id.run}-a${id.attempt}`
			const directory = join(config.stateDir, 'events')
			await mkdir(directory, { recursive: true, mode: 0o700 })
			const path = join(directory, key + '.json')
			if (await readJSON(join(config.stateDir, 'seen', key + '.json'))) {
				await store(path, { ...id, status: 'seen', eventId: event.id })
				return
			}
			const prior = await readJSON(path)
			if (prior?.status === 'notified' || prior?.status === 'no-selection') return
			const run = await api(`actions/runs/${id.run}/attempts/${id.attempt}`, ctx.signal)
			if (run.id !== id.run || run.run_attempt !== id.attempt || run.path !== workflow
				|| run.status !== 'completed' || !['schedule', 'workflow_dispatch'].includes(run.event)) {
				throw new Error('GitHub attempt does not match completion')
			}
			const jobs = []
			for (let page = 1; ; page++) {
				if (page > 10) throw new Error('Observer job pagination exceeds bound')
				const batch = await api(`actions/runs/${id.run}/attempts/${id.attempt}/jobs?per_page=100&page=${page}`, ctx.signal)
				if (!Array.isArray(batch.jobs)) throw new Error('Invalid GitHub jobs response')
				jobs.push(...batch.jobs)
				if (batch.jobs.length < 100) break
			}
			// Superseded queued schedule ticks have no jobs and selected no workload.
			const result = jobs.find(job => job.name === 'result')
			const select = jobs.find(job => job.name === 'select')
			if ((run.event === 'schedule' && run.conclusion === 'cancelled' && jobs.length === 0)
				|| (run.conclusion === 'success' && select?.conclusion === 'success' && result?.conclusion === 'skipped')) {
				await store(path, { ...id, status: 'no-selection', eventId: event.id })
				return
			}
			const marker = `Main-validation-completion:${key}`
			await store(path, { ...id, status: 'pending', eventId: event.id })
			// Recover an ambiguous append without sending a second notification. Amp has no
			// transactional append/idempotency API; the durable pending record survives reloads.
			let appended = false
			if (prior?.status === 'pending') {
				for (let offset = 0; ; offset += 20) {
					ctx.signal.throwIfAborted()
					const messages = await ctx.thread.messages({ full: true, from: 'end', roles: ['user'], limit: 20, offset })
					appended = messages.some(message => message.content.some(block =>
						block.type === 'text' && block.text.startsWith(marker + '\n')))
					if (appended || messages.length < 20) break
				}
			}
			ctx.signal.throwIfAborted()
			if (!appended) await ctx.thread.appendUserMessage({ type: 'user-message', content:
				`${marker}\nA signed completion was verified against GitHub for run ${id.run}, attempt ${id.attempt}. Read .github/main-validation/observer/PROMPT.md and drain the durable events inbox, oldest first. Use existing seen markers and Slack receipts. This notification contains no source identity or workload verdict; establish both from evidence. Do not trigger validation.` })
			await store(path, { ...id, status: 'notified', eventId: event.id })
		})
		tail = work.catch(() => {})
		return work
	}
}

export default async function (amp: PluginAPI) {
	if (!amp.system.workspaceRoot) throw new Error('Observer webhook requires a Golem workspace')
	const root = amp.helpers.filePathFromURI(amp.system.workspaceRoot)
	const configPath = join(root, '.local/main-validation-webhook.json')
	const config = await readJSON(configPath)
	if (!config) return
	if (!isAbsolute(config.stateDir) || !/^T-[0-9a-f-]+$/.test(config.ownerThread)
		|| typeof config.secret !== 'string' || config.secret.length < 32) throw new Error('Invalid observer webhook configuration')
	const api = async (path: string, signal: AbortSignal) => {
		// Amp's connected gh wrapper supplies credentials without exposing a token to plugins.
		try {
			const { stdout } = await promisify(execFile)('gh', ['api', `repos/${repository}/${path}`],
				{ signal, timeout: 10000, maxBuffer: 2 * 1024 * 1024 })
			return JSON.parse(stdout)
		} catch {
			throw new Error('Observer GitHub read failed; check gh access, API availability or handler deadline')
		}
	}
	const registration = await amp.createWebhook({ key: 'main-validation-completed',
		headers: ['x-hub-signature-256', 'x-github-event'], handler: receiver(config, api) })
	// A capability URL is a credential. Never return it in a tool result or log.
	await store(configPath, { ...config, url: registration.url })
}
