import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { spawnSync } from 'node:child_process';
import { createServer as createNetServer } from 'node:net';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { buildPlugin, BINARY, PLUGIN_OUT } from './helpers/pluginBuild.js';

// Sucht einen freien Port
function freePort(): Promise<number> {
  return new Promise((done) => {
    const s = createNetServer();
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => done(port));
    });
  });
}

// Das gebaute Plugin muss ohne die Quellen des Repositorys laufen (SC-008).
describe('Plugin ohne Repository-Quellen', () => {
  let base: string;
  let plugin: string;
  let project: string;
  let home: string;
  let port: number;
  let env: NodeJS.ProcessEnv;

  // Das Binary des kopierten Plugins; genauso startet Claude Code die Hooks, mit Pipes für die
  // Ausgaben. Erbt der abgelöst gestartete Server diese Pipes (unter Windows, wenn `nullradix` die
  // Vererbung seiner Standard-Handles nicht abschaltet), wartet spawnSync bis zum Ende des Servers.
  // Die Zeitgrenze lässt das scheitern: spawnSync blockiert die Ereignisschleife, die Zeitgrenze
  // des Tests greift dann nicht, und der Lauf hinge bis zum Abbruch des CI-Jobs.
  const nullradix = (args: string[], options: { cwd?: string; input?: string } = {}) =>
    spawnSync(join(plugin, 'bin', BINARY), args, { cwd: options.cwd ?? project, env, input: options.input, encoding: 'utf8', windowsHide: true, timeout: 30000 });
  const git = (...args: string[]) => spawnSync('git', args, { cwd: project, encoding: 'utf8', windowsHide: true });

  beforeAll(async () => {
    const built = buildPlugin();
    expect(built.ok, built.message).toBe(true);
    base = realpathSync(mkdtempSync(join(tmpdir(), 'nullradix-plugin-')));
    plugin = join(base, 'installiert', 'nullradix');
    cpSync(PLUGIN_OUT, plugin, { recursive: true }); // Kopie ohne node_modules und ohne Quellen
    project = join(base, 'Projekt ä');
    home = join(base, 'home');
    port = await freePort();
    // NULLRADIX_APP_PATH auf einen Ordner, den es nicht gibt: sonst startet `nullradix board` eine auf
    // diesem Rechner installierte NULLRADIX-App statt des Servers aus dem Plugin, und die schreibt
    // ihren Port in den echten Benutzerordner statt in den dieses Tests
    env = { ...process.env, NULLRADIX_HOME: home, NULLRADIX_PORT: String(port), NULLRADIX_APP_PATH: join(base, 'keine-app'), PATH: process.env.PATH };
    for (const name of ['NULLRADIX_SERVER_ENTRY', 'HARNESS_SERVER_ENTRY', 'NULLRADIX_WEB_DIR', 'HARNESS_WEB_DIR']) delete env[name];
    mkdirSync(project, { recursive: true });
    git('init', '-q');
  }, 300000);

  afterAll(async () => {
    const info = join(home, 'server.json');
    if (existsSync(info)) {
      try {
        process.kill(JSON.parse(readFileSync(info, 'utf8')).pid);
      } catch {
        /* beendet */
      }
    }
    await new Promise((r) => setTimeout(r, 400));
    rmSync(base, { recursive: true, force: true });
  });

  it('zeigt die deutsche Hilfe über das Startskript bin/harness', () => {
    if (process.platform === 'win32') return; // Startdatei für die Bash; unter cmd gilt harness.cmd im Benutzerordner
    const result = spawnSync(join(plugin, 'bin', 'harness'), ['--help'], { cwd: project, env, encoding: 'utf8' });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Befehle:');
    expect(result.stdout).toContain('hook');
  });

  it('richtet ein Projekt ein, startet den eigenen Server und liefert die Oberfläche', async () => {
    git('-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '--allow-empty', '-q', '-m', 'init');
    expect(nullradix(['init']).status).toBe(0);
    const board = nullradix(['--json', 'board', '--no-open', '--port', String(port)]);
    expect(board.stderr).toBe('');
    // Mit --json steht ein Fehler als {"error":…} auf stdout; die Meldung soll im Testlauf sichtbar sein
    expect(board.status, board.stdout).toBe(0);
    expect(JSON.parse(board.stdout).started).toBe(true);
    const health = await (await fetch(`http://127.0.0.1:${port}/api/health`)).json();
    expect(health.name).toBe('ai-harness');
    const page = await (await fetch(`http://127.0.0.1:${port}/`)).text();
    expect(page).toContain('id="root"');
    const asset = /src="(\/assets\/[^"]+\.js)"/.exec(page)?.[1];
    expect(asset).toBeTruthy();
    expect((await fetch(`http://127.0.0.1:${port}${asset}`)).status).toBe(200);
  }, 60000);

  it('schaltet das Plugin nullradix@nullradix-local für das Projekt ein', () => {
    const settings = JSON.parse(readFileSync(join(project, '.claude', 'settings.local.json'), 'utf8'));
    expect(settings.enabledPlugins['nullradix@nullradix-local']).toBe(true);
    expect(existsSync(join(project, '.claude', 'rules', 'nullradix-board.md'))).toBe(true);
    expect(existsSync(join(project, '.claude', 'rules', 'nullradix-docs.md'))).toBe(true);
  });

  it('führt einen Hook aus dem Plugin in unter 3 Sekunden aus und meldet die Session', async () => {
    const start = Date.now();
    const result = nullradix(['hook', 'session-start'], {
      input: JSON.stringify({ session_id: 'plugin-s1', cwd: project }),
    });
    expect(Date.now() - start).toBeLessThan(3000);
    expect(result.status).toBe(0);
    // Ohne board.yaml im NULLRADIX_HOME bleibt der Hook still; die Session wird trotzdem gemeldet
    expect(result.stdout).toBe('');
    const projects = await (await fetch(`http://127.0.0.1:${port}/api/projects`)).json();
    const sessions = await (await fetch(`http://127.0.0.1:${port}/api/projects/${projects[0].id}/sessions`)).json();
    expect(sessions.map((s: any) => s.id)).toEqual(['plugin-s1']);
  }, 30000);

  it('bleibt in einem nicht eingerichteten Projekt wirkungslos', () => {
    const bare = join(base, 'ohne-init');
    spawnSync('git', ['init', '-q', bare], { windowsHide: true });
    const result = nullradix(['hook', 'session-start'], {
      cwd: bare,
      input: JSON.stringify({ session_id: 'x', cwd: bare }),
    });
    expect(result.status).toBe(0);
    expect(result.stdout).toBe('');
    expect(result.stderr).toBe('');
    expect(existsSync(join(bare, '.git', 'harness'))).toBe(false);
  });
});
