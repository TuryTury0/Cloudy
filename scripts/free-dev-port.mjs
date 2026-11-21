// Runs before the dev server. On Windows a vite from an earlier run can outlive its `tauri dev` (child processes
// are not stopped with their parent) and keep port 1420, so every later run fails with "Port 1420 is already in
// use". If the port is held by this project's own vite, that leftover is stopped; anything else is reported.
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.argv[2] || 1420);
const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..').toLowerCase();

if (process.platform !== 'win32') process.exit(0);

const powershell = (command) => execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', command], { encoding: 'utf8' }).trim();

let owner;
try {
  owner = powershell(
    `$c = Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1; ` +
      `if ($c) { $p = Get-CimInstance Win32_Process -Filter ("ProcessId=" + $c.OwningProcess); $c.OwningProcess; $p.Name; $p.CommandLine }`
  );
} catch {
  process.exit(0); // Could not check; vite will report a busy port itself.
}
if (!owner) process.exit(0);

const [pid, name, ...rest] = owner.split(/\r?\n/);
const commandLine = rest.join(' ').toLowerCase().replaceAll('\\\\', '\\');
if (commandLine.includes('vite') && commandLine.includes(project)) {
  execFileSync('taskkill', ['/PID', pid, '/T', '/F'], { stdio: 'ignore' });
  console.log(`Stopped a leftover dev server from an earlier run (pid ${pid}) that was holding port ${port}.`);
  process.exit(0);
}
console.error(`Port ${port} is used by another program: ${name} (pid ${pid}). Close it and run again.`);
process.exit(1);
