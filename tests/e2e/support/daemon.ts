import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { closeSync, openSync, readFileSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { promisify } from 'node:util';
import { list, record, text } from './json';
import { fakeClaudeBinary, nullradixBinary, webDist } from './paths';

const run = promisify(execFile);

const STARTUP_MS = 60_000;
const WIND_DOWN_MS = 30_000;
const POLL_MS = 50;

// Variables a developer's shell may carry that would change what the daemon does.
const SCRUBBED = [
  'NULLRADIX_PORT',
  'NULLRADIX_BIND',
  'NULLRADIX_LOG',
  'NULLRADIX_WEB_DIR',
  'NULLRADIX_DEBUG',
  'NULLRADIX_TICKET',
  'NULLRADIX_RUN',
  'NULLRADIX_HOOK_DEADLINE_MS',
  'CLAUDE_CODE_ENTRYPOINT',
  'CLAUDE_ENV_FILE',
  'ANTHROPIC_API_KEY',
  'FAKE_CLAUDE_MODE',
  'FAKE_CLAUDE_REPLY',
  'FAKE_CLAUDE_REPLY_FILE',
  'FAKE_CLAUDE_MERGE',
  'NULLRADIX_THEME_CATALOG_LOCAL',
  'NULLRADIX_SESSION',
  'GIT_DIR',
  'GIT_WORK_TREE',
  'GIT_INDEX_FILE',
  'GIT_COMMON_DIR',
];

const REQUEST_MS = 10_000;
const KILL_MS = 10_000;

const ACTIVE = new Set(['starting', 'running']);

// A daemon with its own home and work folder; nothing of the real profile is touched.
export class Daemon {
  readonly home: string;
  readonly work: string;
  private readonly gitConfig: string;
  private readonly mode: string;
  private child: ChildProcess | undefined;
  private port = 0;
  private token = '';
  private ended: Promise<void> = Promise.resolve();

  private constructor(home: string, mode: string) {
    this.home = home;
    this.work = join(home, 'work');
    this.gitConfig = join(home, 'gitconfig');
    this.mode = mode;
  }

  static async start(mode = 'review'): Promise<Daemon> {
    const home = await mkdtemp(join(tmpdir(), 'nullradix-e2e-'));
    const daemon = new Daemon(home, mode);
    await mkdir(daemon.work, { recursive: true });
    // an empty global config keeps the machine's identity, signing and line endings out of git
    await writeFile(daemon.gitConfig, '');
    try {
      await daemon.launch();
    } catch (error) {
      await daemon.kill();
      await rm(home, { recursive: true, force: true }).catch(() => undefined);
      throw error;
    }
    return daemon;
  }

  get baseUrl(): string {
    return `http://127.0.0.1:${String(this.port)}`;
  }

  // The environment of the daemon, the CLI and every git call of a test.
  env(): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = { ...process.env };
    for (const name of SCRUBBED) Reflect.deleteProperty(env, name);
    return {
      ...env,
      NULLRADIX_HOME: this.home,
      NULLRADIX_PROFILE: 'dev',
      NULLRADIX_PERSON: 'tester',
      NULLRADIX_CLAUDE: fakeClaudeBinary,
      FAKE_CLAUDE_MODE: this.mode,
      CLAUDE_CONFIG_DIR: join(this.work, 'claude-config'),
      GIT_CONFIG_GLOBAL: this.gitConfig,
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_AUTHOR_NAME: 't',
      GIT_AUTHOR_EMAIL: 't@example.com',
      GIT_COMMITTER_NAME: 't',
      GIT_COMMITTER_EMAIL: 't@example.com',
    };
  }

  // Starts the process; a restart keeps the port, so an open page reconnects to the same origin.
  private async launch(): Promise<void> {
    const log = openSync(join(this.home, 'daemon.stderr'), 'a');
    const child = spawn(
      nullradixBinary,
      ['serve', '--port', String(this.port), '--web-dir', webDist],
      { env: this.env(), stdio: ['ignore', 'ignore', log], windowsHide: true },
    );
    closeSync(log);
    const state = { stopped: false };
    this.ended = new Promise((resolve) => {
      child.once('exit', () => {
        state.stopped = true;
        resolve();
      });
    });
    this.child = child;
    const discovery = join(this.home, 'data', 'daemon.json');
    const deadline = Date.now() + STARTUP_MS;
    for (;;) {
      if (state.stopped) throw new Error(`the daemon stopped at startup: ${this.stderr()}`);
      if (Date.now() > deadline) throw new Error('the daemon did not start in time');
      const address = await this.discovered(discovery, child.pid);
      if (address !== undefined && (await this.healthy(address))) {
        this.port = Number(address.slice(address.lastIndexOf(':') + 1));
        break;
      }
      await sleep(POLL_MS);
    }
    const token = await readFile(join(this.home, 'config', 'cli-token'), 'utf8');
    this.token = token.trim();
  }

  private async discovered(file: string, pid: number | undefined): Promise<string | undefined> {
    try {
      const found = record(JSON.parse(await readFile(file, 'utf8')), 'daemon.json');
      return found['pid'] === pid && typeof found['address'] === 'string'
        ? found['address']
        : undefined;
    } catch {
      return undefined;
    }
  }

  private async healthy(address: string): Promise<boolean> {
    try {
      return (
        await fetch(`http://${address}/api/v1/health`, { signal: AbortSignal.timeout(REQUEST_MS) })
      ).ok;
    } catch {
      return false;
    }
  }

  private stderr(): string {
    try {
      return readFileSync(join(this.home, 'daemon.stderr'), 'utf8').trim();
    } catch {
      return '';
    }
  }

  // Kills the process without a clean shutdown; waits for the exit, forcing it after a while.
  private async kill(): Promise<void> {
    this.child?.kill('SIGKILL');
    await Promise.race([this.ended, sleep(KILL_MS)]);
  }

  // Crashes the daemon (SIGKILL) and starts it again on the same home and port.
  async restart(): Promise<void> {
    await this.kill();
    await this.launch();
  }

  async api(method: string, path: string, body?: unknown): Promise<unknown> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      signal: AbortSignal.timeout(REQUEST_MS),
      headers: {
        Authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    const reply = await response.text();
    if (!response.ok) {
      throw new Error(`${method} ${path} gave ${String(response.status)}: ${reply}`);
    }
    return reply === '' ? null : (JSON.parse(reply) as unknown);
  }

  // The path that opens a browser session; the code works once.
  async handoff(): Promise<string> {
    return text(await this.api('POST', '/api/v1/auth/handoffs'), 'path');
  }

  // The CLI of this home, run in the work folder; returns what it printed.
  async cli(args: string[]): Promise<string> {
    const { stdout } = await run(nullradixBinary, args, { cwd: this.work, env: this.env() });
    return stdout;
  }

  async git(cwd: string, args: string[]): Promise<string> {
    const { stdout } = await run('git', args, { cwd, env: this.env() });
    return stdout;
  }

  // Cancels what still runs, since run supervisors outlive the daemon, then removes the home.
  async stop(): Promise<void> {
    await this.windDown();
    await this.kill();
    try {
      await rm(this.home, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
    } catch (error) {
      console.warn(`cannot remove ${this.home}: ${String(error)}`);
    }
  }

  private async active(): Promise<string[]> {
    try {
      const runs = list(await this.api('GET', '/api/v1/runs?limit=500'), 'items');
      return runs
        .map((item) => record(item, 'a run'))
        .filter((item) => typeof item['status'] === 'string' && ACTIVE.has(item['status']))
        .map((item) => text(item, 'id'));
    } catch {
      return [];
    }
  }

  private async windDown(): Promise<void> {
    // no pass may start a new run while the old ones end
    await this.api('PATCH', '/api/v1/dispatcher', { enabled: false }).catch(() => null);
    for (const id of await this.active()) {
      await this.api('POST', `/api/v1/runs/${id}/cancel`).catch(() => null);
    }
    const deadline = Date.now() + WIND_DOWN_MS;
    while (Date.now() < deadline && (await this.active()).length > 0) await sleep(100);
  }
}
